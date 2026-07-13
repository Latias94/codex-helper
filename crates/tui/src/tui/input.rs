mod history_bridge;
mod normal;
mod transcript;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use super::model::{ProviderOption, Snapshot};
use super::state::UiState;
use super::types::Overlay;
pub(in crate::tui) use normal::{apply_codex_switch, codex_switch_intent_for_key};
use normal::{apply_page_shortcuts, handle_key_normal, toggle_language};
use transcript::handle_key_session_transcript;

pub(in crate::tui) fn should_accept_key_event(event: &KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
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
                toggle_language(ctx.ui);
                true
            }
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests;
