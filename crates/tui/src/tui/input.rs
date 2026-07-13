mod history_bridge;
mod normal;
mod transcript;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::model::{ProviderOption, Snapshot};
use super::state::UiState;
use super::types::Overlay;
pub(in crate::tui) use normal::export_selected_stats_report;
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

pub(in crate::tui) struct KeyEventContext<'a> {
    pub(in crate::tui) providers: &'a mut Vec<ProviderOption>,
    pub(in crate::tui) ui: &'a mut UiState,
    pub(in crate::tui) snapshot: &'a Snapshot,
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
    }
}

#[cfg(test)]
mod tests;
