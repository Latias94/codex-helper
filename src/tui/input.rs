use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::state::ProxyState;

use super::model::{ProviderOption, Snapshot, filtered_requests_len, now_ms};
use super::state::{UiState, adjust_table_selection};
use super::types::{EffortChoice, Focus, Overlay, Page};

pub(in crate::tui) fn should_accept_key_event(event: &KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(in crate::tui) async fn handle_key_event(
    state: &ProxyState,
    providers: &[ProviderOption],
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    if ui.overlay == Overlay::None && apply_page_shortcuts(ui, key.code) {
        return true;
    }

    match ui.overlay {
        Overlay::None => handle_key_normal(state, providers, ui, snapshot, key).await,
        Overlay::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                ui.overlay = Overlay::None;
                true
            }
            _ => false,
        },
        Overlay::EffortMenu => handle_key_effort_menu(state, ui, snapshot, key).await,
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            handle_key_provider_menu(state, providers, ui, snapshot, key).await
        }
    }
}

fn apply_page_shortcuts(ui: &mut UiState, code: KeyCode) -> bool {
    let page = match code {
        KeyCode::Char('1') => Some(Page::Dashboard),
        KeyCode::Char('2') => Some(Page::Configs),
        KeyCode::Char('3') => Some(Page::Sessions),
        KeyCode::Char('4') => Some(Page::Requests),
        KeyCode::Char('5') => Some(Page::Settings),
        _ => None,
    };
    if let Some(p) = page {
        ui.page = p;
        if ui.page == Page::Configs {
            ui.focus = Focus::Configs;
        } else if ui.page == Page::Dashboard && ui.focus == Focus::Configs {
            ui.focus = Focus::Sessions;
        }
        return true;
    }
    false
}

async fn apply_effort_override(state: &ProxyState, sid: String, effort: Option<String>) {
    let now = now_ms();
    if let Some(eff) = effort {
        state.set_session_effort_override(sid, eff, now).await;
    } else {
        state.clear_session_effort_override(&sid).await;
    }
}

async fn apply_session_provider_override(state: &ProxyState, sid: String, cfg: Option<String>) {
    let now = now_ms();
    if let Some(cfg) = cfg {
        state.set_session_config_override(sid, cfg, now).await;
    } else {
        state.clear_session_config_override(&sid).await;
    }
}

async fn apply_global_provider_override(state: &ProxyState, cfg: Option<String>) {
    if let Some(cfg) = cfg {
        state.set_global_config_override(cfg).await;
    } else {
        state.clear_global_config_override().await;
    }
}

async fn handle_key_normal(
    state: &ProxyState,
    providers: &[ProviderOption],
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        ui.should_exit = true;
        return true;
    }

    match key.code {
        KeyCode::Char('q') => {
            ui.should_exit = true;
            true
        }
        KeyCode::Char('?') => {
            ui.overlay = Overlay::Help;
            true
        }
        KeyCode::Tab => {
            if ui.page == Page::Dashboard {
                ui.focus = match ui.focus {
                    Focus::Sessions => Focus::Requests,
                    Focus::Requests => Focus::Sessions,
                    Focus::Configs => Focus::Sessions,
                };
            } else if ui.page == Page::Configs {
                ui.focus = Focus::Configs;
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Configs => {
            if let Some(next) = adjust_table_selection(&mut ui.configs_table, -1, providers.len()) {
                ui.selected_config_idx = next;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Configs => {
            if let Some(next) = adjust_table_selection(&mut ui.configs_table, 1, providers.len()) {
                ui.selected_config_idx = next;
                return true;
            }
            false
        }
        KeyCode::Enter if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            apply_global_provider_override(state, Some(pvd.name.clone())).await;
            ui.toast = Some((format!("global cfg override: {}", pvd.name), Instant::now()));
            true
        }
        KeyCode::Backspace | KeyCode::Delete if ui.page == Page::Configs => {
            apply_global_provider_override(state, None).await;
            ui.toast = Some(("global cfg override: <clear>".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('o') if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.toast = Some((
                    "session cfg override: <no session>".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            apply_session_provider_override(state, sid, Some(pvd.name.clone())).await;
            ui.toast = Some((
                format!("session cfg override: {}", pvd.name),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('O') if ui.page == Page::Configs => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.toast = Some((
                    "session cfg override: <no session>".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            apply_session_provider_override(state, sid, None).await;
            ui.toast = Some(("session cfg override: <clear>".to_string(), Instant::now()));
            true
        }
        KeyCode::Up | KeyCode::Char('k') => match ui.focus {
            Focus::Sessions => {
                if let Some(next) =
                    adjust_table_selection(&mut ui.sessions_table, -1, snapshot.rows.len())
                {
                    ui.selected_session_idx = next;
                    ui.selected_session_id =
                        snapshot.rows.get(next).and_then(|r| r.session_id.clone());
                    ui.selected_request_idx = 0;
                    let req_len = filtered_requests_len(snapshot, next);
                    ui.requests_table
                        .select(if req_len == 0 { None } else { Some(0) });
                    return true;
                }
                false
            }
            Focus::Requests => {
                let filtered_len = filtered_requests_len(snapshot, ui.selected_session_idx);
                if let Some(next) = adjust_table_selection(&mut ui.requests_table, -1, filtered_len)
                {
                    ui.selected_request_idx = next;
                    return true;
                }
                false
            }
            Focus::Configs => false,
        },
        KeyCode::Down | KeyCode::Char('j') => match ui.focus {
            Focus::Sessions => {
                if let Some(next) =
                    adjust_table_selection(&mut ui.sessions_table, 1, snapshot.rows.len())
                {
                    ui.selected_session_idx = next;
                    ui.selected_session_id =
                        snapshot.rows.get(next).and_then(|r| r.session_id.clone());
                    ui.selected_request_idx = 0;
                    let req_len = filtered_requests_len(snapshot, next);
                    ui.requests_table
                        .select(if req_len == 0 { None } else { Some(0) });
                    return true;
                }
                false
            }
            Focus::Requests => {
                let filtered_len = filtered_requests_len(snapshot, ui.selected_session_idx);
                if let Some(next) = adjust_table_selection(&mut ui.requests_table, 1, filtered_len)
                {
                    ui.selected_request_idx = next;
                    return true;
                }
                false
            }
            Focus::Configs => false,
        },
        KeyCode::Enter => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            if snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.as_deref())
                .is_none()
            {
                return false;
            }

            ui.overlay = Overlay::EffortMenu;
            let current = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.override_effort.as_deref())
                .unwrap_or("");
            ui.effort_menu_idx = match current {
                "low" => 1,
                "medium" => 2,
                "high" => 3,
                "xhigh" => 4,
                _ => 0,
            };
            true
        }
        KeyCode::Char('l') | KeyCode::Char('m') | KeyCode::Char('h') | KeyCode::Char('X') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            let eff = match key.code {
                KeyCode::Char('l') => Some("low"),
                KeyCode::Char('m') => Some("medium"),
                KeyCode::Char('h') => Some("high"),
                KeyCode::Char('X') => Some("xhigh"),
                _ => None,
            }
            .map(|s| s.to_string());
            apply_effort_override(state, sid, eff.clone()).await;
            ui.toast = Some((
                format!("effort override: {}", eff.as_deref().unwrap_or("<clear>")),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('x') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            apply_effort_override(state, sid, None).await;
            ui.toast = Some(("effort override cleared".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('p') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            let current = snapshot
                .config_overrides
                .get(&sid)
                .map(|s| s.as_str())
                .unwrap_or("");
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.overlay = Overlay::ProviderMenuSession;
            true
        }
        KeyCode::Char('P') => {
            let current = snapshot.global_override.as_deref().unwrap_or("");
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.overlay = Overlay::ProviderMenuGlobal;
            true
        }
        _ => false,
    }
}

async fn handle_key_effort_menu(
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
            ui.toast = Some((format!("effort set: {}", choice.label()), Instant::now()));
            true
        }
        _ => false,
    }
}

async fn handle_key_provider_menu(
    state: &ProxyState,
    providers: &[ProviderOption],
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
            ui.provider_menu_idx = ui.provider_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = providers.len();
            ui.provider_menu_idx = (ui.provider_menu_idx + 1).min(max);
            true
        }
        KeyCode::Enter => {
            let idx = ui.provider_menu_idx;
            let chosen = if idx == 0 {
                None
            } else {
                providers.get(idx - 1).map(|p| p.name.clone())
            };

            match ui.overlay {
                Overlay::ProviderMenuGlobal => {
                    apply_global_provider_override(state, chosen.clone()).await;
                    ui.toast = Some((
                        format!(
                            "global cfg override: {}",
                            chosen.as_deref().unwrap_or("<clear>")
                        ),
                        Instant::now(),
                    ));
                }
                Overlay::ProviderMenuSession => {
                    let Some(sid) = snapshot
                        .rows
                        .get(ui.selected_session_idx)
                        .and_then(|r| r.session_id.clone())
                    else {
                        ui.overlay = Overlay::None;
                        return true;
                    };
                    apply_session_provider_override(state, sid, chosen.clone()).await;
                    ui.toast = Some((
                        format!(
                            "session cfg override: {}",
                            chosen.as_deref().unwrap_or("<clear>")
                        ),
                        Instant::now(),
                    ));
                }
                _ => {}
            }

            ui.overlay = Overlay::None;
            true
        }
        _ => false,
    }
}
