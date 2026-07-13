use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use codex_helper_core::codex_switch::CodexSwitchIntent;

use super::{KeyEventContext, handle_key_event};
use crate::tui::Language;
use crate::tui::input::normal::codex_switch_intent_for_key;
use crate::tui::model::{ProviderOption, Snapshot};
use crate::tui::state::UiState;
use crate::tui::types::{Focus, Overlay, Page};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

async fn press(ui: &mut UiState, snapshot: &Snapshot, code: KeyCode) -> bool {
    let mut providers = Vec::<ProviderOption>::new();
    handle_key_event(
        KeyEventContext {
            providers: &mut providers,
            ui,
            snapshot,
        },
        key(code),
    )
    .await
}

#[tokio::test]
async fn page_navigation_and_local_view_controls_remain_available() {
    let mut ui = UiState::default();
    let snapshot = Snapshot::default();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('5')).await);
    assert_eq!(ui.page, Page::Stats);
    assert!(press(&mut ui, &snapshot, KeyCode::Tab).await);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('?')).await);
    assert_eq!(ui.overlay, Overlay::Help);
    assert!(press(&mut ui, &snapshot, KeyCode::Esc).await);
    assert_eq!(ui.overlay, Overlay::None);
}

#[tokio::test]
async fn hotkey_two_opens_routing_with_provider_focus() {
    let mut ui = UiState::default();
    let snapshot = Snapshot::default();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('2')).await);
    assert_eq!(ui.page, Page::Routing);
    assert_eq!(ui.focus, Focus::Providers);
}

#[tokio::test]
async fn stats_refresh_requests_a_new_operator_read_model() {
    let mut ui = UiState {
        page: Page::Stats,
        ..UiState::default()
    };
    let snapshot = Snapshot::default();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('g')).await);
    assert!(ui.needs_snapshot_refresh);
}

#[tokio::test]
async fn language_toggle_is_scoped_to_the_current_tui_state() {
    let mut ui = UiState {
        language: Language::En,
        ..UiState::default()
    };
    let snapshot = Snapshot::default();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('L')).await);

    assert_eq!(ui.language, Language::Zh);
    assert!(
        ui.toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("TUI"))
    );
}

#[tokio::test]
async fn removed_runtime_mutation_keys_are_inert() {
    let snapshot = Snapshot::default();
    let cases = [
        (Page::Settings, vec!['C', 'X', 'Y', 'R', 'p', 'P']),
        (Page::Routing, vec!['r', 'g', 'h', 'H', 'c', 'C', 'o', 'O']),
        (
            Page::Sessions,
            vec!['b', 'M', 'f', 'R', 'l', 'm', 'h', 'X', 'x', 'p', 'P'],
        ),
    ];

    for (page, keys) in cases {
        for code in keys {
            let mut ui = UiState {
                page,
                ..UiState::default()
            };
            assert!(
                !press(&mut ui, &snapshot, KeyCode::Char(code)).await,
                "{code:?} must be inert on {page:?}"
            );
            assert_eq!(ui.overlay, Overlay::None);
        }
    }
}

#[tokio::test]
async fn provider_details_remain_a_read_only_overlay() {
    let mut ui = UiState {
        page: Page::Routing,
        ..UiState::default()
    };
    let snapshot = Snapshot::default();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('i')).await);
    assert_eq!(ui.overlay, Overlay::ProviderInfo);
    assert!(press(&mut ui, &snapshot, KeyCode::Esc).await);
    assert_eq!(ui.overlay, Overlay::None);
}

#[test]
fn settings_n_o_keys_keep_the_explicit_local_codex_switch_contract() {
    assert!(matches!(
        codex_switch_intent_for_key(KeyCode::Char('n'), 4321),
        Some(CodexSwitchIntent::On { .. })
    ));
    assert_eq!(
        codex_switch_intent_for_key(KeyCode::Char('o'), 4321),
        Some(CodexSwitchIntent::Off)
    );
    assert_eq!(codex_switch_intent_for_key(KeyCode::Char('x'), 4321), None);
}
