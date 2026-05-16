use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::storage::load_config;
use crate::dashboard_core::ControlProfileOption;
use crate::proxy::ProxyService;
use crate::state::ProxyState;
use crate::tui::i18n;
use crate::tui::model::{Snapshot, now_ms};
use crate::tui::state::UiState;
use crate::tui::types::Overlay;

pub(in crate::tui) async fn refresh_profile_control_state(
    ui: &mut UiState,
    proxy: &ProxyService,
) -> anyhow::Result<()> {
    let response = proxy.profiles().await;
    ui.configured_default_profile = response.configured_default_profile.clone();
    ui.effective_default_profile = response.default_profile.clone();
    ui.runtime_default_profile_override =
        if response.default_profile != response.configured_default_profile {
            response.default_profile.clone()
        } else {
            None
        };
    ui.profile_options = response.profiles;
    Ok(())
}

pub(super) fn default_profile_menu_idx(
    profiles: &[ControlProfileOption],
    binding_profile_name: Option<&str>,
) -> usize {
    match binding_profile_name {
        Some(name) => profiles
            .iter()
            .position(|profile| profile.name == name)
            .map(|idx| idx + 1)
            .unwrap_or(0),
        None => usize::from(!profiles.is_empty()),
    }
}

pub(super) fn runtime_default_profile_menu_idx(
    profiles: &[ControlProfileOption],
    runtime_default_profile_override: Option<&str>,
) -> usize {
    match runtime_default_profile_override {
        Some(name) => default_profile_menu_idx(profiles, Some(name)),
        None => 0,
    }
}

async fn apply_runtime_default_profile(
    proxy: &ProxyService,
    profile_name: Option<String>,
) -> anyhow::Result<()> {
    proxy.set_runtime_default_profile(profile_name).await?;
    Ok(())
}

async fn apply_persisted_default_profile(
    proxy: &ProxyService,
    profile_name: Option<String>,
) -> anyhow::Result<()> {
    proxy.set_persisted_default_profile(profile_name).await?;
    Ok(())
}

fn default_profile_label(value: Option<&str>, fallback: &str) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn profile_menu_max_idx(profiles: &[ControlProfileOption]) -> usize {
    profiles.len()
}

async fn apply_session_profile(
    state: &ProxyState,
    service_name: &str,
    sid: String,
    profile_name: String,
) -> anyhow::Result<()> {
    let cfg = load_config().await?;
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    state
        .apply_session_profile_binding(service_name, mgr, sid, profile_name, now_ms())
        .await
}

pub(super) async fn handle_key_profile_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.profile_menu_idx = ui.profile_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = profile_menu_max_idx(&ui.profile_options);
            ui.profile_menu_idx = (ui.profile_menu_idx + 1).min(max);
            true
        }
        KeyCode::Enter => {
            let chosen = if ui.profile_menu_idx == 0 {
                None
            } else {
                ui.profile_options
                    .get(ui.profile_menu_idx.saturating_sub(1))
                    .map(|profile| profile.name.clone())
            };

            match ui.overlay {
                Overlay::ProfileMenuSession => {
                    let Some(sid) = snapshot
                        .rows
                        .get(ui.selected_session_idx)
                        .and_then(|row| row.session_id.clone())
                    else {
                        ui.overlay = Overlay::None;
                        return true;
                    };

                    if let Some(profile_name) = chosen {
                        match apply_session_profile(
                            state,
                            ui.service_name,
                            sid,
                            profile_name.clone(),
                        )
                        .await
                        {
                            Ok(()) => {
                                ui.needs_snapshot_refresh = true;
                                ui.toast = Some((
                                    format!(
                                        "{}: {profile_name}",
                                        i18n::label(ui.language, "profile applied")
                                    ),
                                    Instant::now(),
                                ));
                            }
                            Err(err) => {
                                ui.toast = Some((
                                    format!(
                                        "{}: {err}",
                                        i18n::label(ui.language, "profile apply failed")
                                    ),
                                    Instant::now(),
                                ));
                            }
                        }
                    } else {
                        state.clear_session_binding(&sid).await;
                        ui.needs_snapshot_refresh = true;
                        ui.toast = Some((
                            i18n::label(ui.language, "profile binding cleared").to_string(),
                            Instant::now(),
                        ));
                    }
                }
                Overlay::ProfileMenuDefaultRuntime => {
                    match apply_runtime_default_profile(proxy, chosen.clone()).await {
                        Ok(()) => match refresh_profile_control_state(ui, proxy).await {
                            Ok(()) => {
                                ui.toast = Some((
                                    format!(
                                        "{}: {}",
                                        i18n::label(ui.language, "runtime default profile"),
                                        default_profile_label(
                                            ui.runtime_default_profile_override.as_deref(),
                                            i18n::label(ui.language, "<configured fallback>"),
                                        )
                                    ),
                                    Instant::now(),
                                ));
                            }
                            Err(err) => {
                                ui.toast = Some((
                                    format!(
                                        "{}: {err}",
                                        i18n::label(
                                            ui.language,
                                            "runtime default profile refresh failed"
                                        )
                                    ),
                                    Instant::now(),
                                ));
                            }
                        },
                        Err(err) => {
                            ui.toast = Some((
                                format!(
                                    "{}: {err}",
                                    i18n::label(
                                        ui.language,
                                        "runtime default profile apply failed"
                                    )
                                ),
                                Instant::now(),
                            ));
                        }
                    }
                }
                Overlay::ProfileMenuDefaultPersisted => {
                    match apply_persisted_default_profile(proxy, chosen.clone()).await {
                        Ok(()) => match refresh_profile_control_state(ui, proxy).await {
                            Ok(()) => {
                                ui.toast = Some((
                                    format!(
                                        "{}: {}",
                                        i18n::label(ui.language, "configured default profile"),
                                        default_profile_label(
                                            ui.configured_default_profile.as_deref(),
                                            i18n::label(ui.language, "<none>"),
                                        )
                                    ),
                                    Instant::now(),
                                ));
                            }
                            Err(err) => {
                                ui.toast = Some((
                                    format!(
                                        "{}: {err}",
                                        i18n::label(
                                            ui.language,
                                            "configured default profile refresh failed"
                                        )
                                    ),
                                    Instant::now(),
                                ));
                            }
                        },
                        Err(err) => {
                            ui.toast = Some((
                                format!(
                                    "{}: {err}",
                                    i18n::label(
                                        ui.language,
                                        "configured default profile apply failed"
                                    )
                                ),
                                Instant::now(),
                            ));
                        }
                    }
                }
                _ => {}
            }
            ui.overlay = Overlay::None;
            true
        }
        _ => false,
    }
}
