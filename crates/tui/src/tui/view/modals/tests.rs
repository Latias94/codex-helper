use super::{
    current_page_help_lines, help_quit_line_for_tests, help_text_for_tests,
    language_help_line_for_tests, transcript_max_scroll,
};
use crate::dashboard_core::OperatorActionCapabilities;
use crate::tui::Language;
use crate::tui::model::Palette;
use crate::tui::state::{RuntimeConnectionKind, UiState};
use crate::tui::types::Page;
use ratatui::prelude::Line;

fn ui_for(page: Page, runtime_connection: RuntimeConnectionKind) -> UiState {
    UiState {
        page,
        language: Language::En,
        runtime_connection,
        ..UiState::default()
    }
}

#[test]
fn local_routing_help_advertises_refresh_preference_and_clear() {
    let ui = ui_for(Page::Routing, RuntimeConnectionKind::Integrated);
    let lines = current_page_help_lines(&ui, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("Current page: Routing"), "{text}");
    assert!(text.contains("endpoint candidate"), "{text}");
    assert!(text.contains("Enter"), "{text}");
    assert!(text.contains("new-session preference"), "{text}");
    assert!(text.contains("Backspace"), "{text}");
    assert!(text.contains("force-refresh all balances"), "{text}");
}

#[test]
fn remote_routing_help_remains_read_only() {
    let ui = ui_for(Page::Routing, RuntimeConnectionKind::RemoteObserver);
    let lines = current_page_help_lines(&ui, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("inspect the current provider"), "{text}");
    assert!(
        !text.contains("new-session preference and endpoint actions"),
        "{text}"
    );
    assert!(!text.contains("Backspace"), "{text}");
}

#[test]
fn usage_help_keeps_quota_navigation_refresh_and_report_export() {
    let ui = ui_for(Page::Stats, RuntimeConnectionKind::Integrated);
    let lines = current_page_help_lines(&ui, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(
        text.contains("pool / project / provider / endpoint"),
        "{text}"
    );
    assert!(text.contains("move the active"), "{text}");
    assert!(text.contains("refresh"), "{text}");
    assert!(text.contains("export and copy"), "{text}");
}

#[test]
fn integrated_settings_help_advertises_available_controls() {
    let ui = ui_for(Page::Settings, RuntimeConnectionKind::Integrated);
    let lines = current_page_help_lines(&ui, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("scroll settings"), "{text}");
    assert!(text.contains("configured profile"), "{text}");
    assert!(text.contains("reload runtime"), "{text}");
    assert!(text.contains("capability diagnostics"), "{text}");
    assert!(text.contains("live smoke"), "{text}");
    assert!(text.contains("n/o"), "{text}");
    assert!(text.contains("switch"), "{text}");
    assert!(text.contains("B/I/F/V/D"), "{text}");
}

#[test]
fn remote_settings_help_is_strictly_read_only() {
    let ui = ui_for(Page::Settings, RuntimeConnectionKind::RemoteObserver);
    let lines = current_page_help_lines(&ui, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("operator bundle is read-only"), "{text}");
    for blocked in [
        "  p/P        ",
        "reload runtime",
        "  C          ",
        "X/Y",
        "n/o",
        "B/I/F/V/D",
    ] {
        assert!(!text.contains(blocked), "unexpected {blocked:?} in {text}");
    }
}

#[test]
fn local_attached_settings_help_follows_each_capability() {
    let ui = UiState {
        page: Page::Settings,
        language: Language::En,
        runtime_connection: RuntimeConnectionKind::LocalAttached,
        operator_action_capabilities: OperatorActionCapabilities {
            reload_runtime: true,
            inspect_relay_capabilities: true,
            ..OperatorActionCapabilities::default()
        },
        ..UiState::default()
    };
    let text = help_text_for_tests(&current_page_help_lines(&ui, Palette::default()));

    assert!(text.contains("reload runtime"), "{text}");
    assert!(text.contains("capability diagnostics"), "{text}");
    assert!(text.contains("n/o"), "{text}");
    assert!(!text.contains("  p/P        "), "{text}");
    assert!(!text.contains("X/Y"), "{text}");
}

#[test]
fn language_help_matches_runtime_persistence_capability() {
    for runtime_connection in [
        RuntimeConnectionKind::Integrated,
        RuntimeConnectionKind::LocalAttached,
    ] {
        let ui = ui_for(Page::Settings, runtime_connection);
        let line = language_help_line_for_tests(&ui);
        assert!(line.contains("save it to config.toml"), "{line}");
    }

    let remote = ui_for(Page::Settings, RuntimeConnectionKind::RemoteObserver);
    let line = language_help_line_for_tests(&remote);
    assert!(line.contains("current TUI session only"), "{line}");
}

#[test]
fn remote_help_keeps_local_history_browsing_without_claiming_runtime_bridge_actions() {
    let remote_sessions = help_text_for_tests(&current_page_help_lines(
        &ui_for(Page::Sessions, RuntimeConnectionKind::RemoteObserver),
        Palette::default(),
    ));
    assert!(
        remote_sessions.contains("related Requests"),
        "{remote_sessions}"
    );
    assert!(
        !remote_sessions.contains("full-screen transcript"),
        "{remote_sessions}"
    );
    assert!(
        !remote_sessions.contains("jump to History"),
        "{remote_sessions}"
    );

    let remote_requests = help_text_for_tests(&current_page_help_lines(
        &ui_for(Page::Requests, RuntimeConnectionKind::RemoteObserver),
        Palette::default(),
    ));
    assert!(
        remote_requests.contains("related Sessions"),
        "{remote_requests}"
    );
    assert!(
        !remote_requests.contains("jump to History"),
        "{remote_requests}"
    );

    let remote_history = help_text_for_tests(&current_page_help_lines(
        &ui_for(Page::History, RuntimeConnectionKind::RemoteObserver),
        Palette::default(),
    ));
    assert!(
        remote_history.contains("full-screen transcript"),
        "{remote_history}"
    );
    assert!(
        !remote_history.contains("jump to Sessions / Requests"),
        "{remote_history}"
    );

    let remote_recent = help_text_for_tests(&current_page_help_lines(
        &ui_for(Page::Recent, RuntimeConnectionKind::RemoteObserver),
        Palette::default(),
    ));
    assert!(remote_recent.contains("jump to History"), "{remote_recent}");
    assert!(
        !remote_recent.contains("jump to Sessions / Requests"),
        "{remote_recent}"
    );

    let integrated_sessions = help_text_for_tests(&current_page_help_lines(
        &ui_for(Page::Sessions, RuntimeConnectionKind::Integrated),
        Palette::default(),
    ));
    assert!(
        integrated_sessions.contains("full-screen transcript"),
        "{integrated_sessions}"
    );
    assert!(
        integrated_sessions.contains("jump to History"),
        "{integrated_sessions}"
    );
}

#[test]
fn help_quit_line_distinguishes_integrated_and_attached_lifecycles() {
    let attached = help_quit_line_for_tests(Language::En, true);
    let integrated = help_quit_line_for_tests(Language::En, false);

    assert!(
        attached.contains("exit attached console only"),
        "{attached}"
    );
    assert!(
        attached.contains("keep resident proxy running"),
        "{attached}"
    );
    assert!(integrated.contains("request shutdown"), "{integrated}");
    assert!(!attached.contains("shutdown"), "{attached}");
}

#[test]
fn transcript_scroll_limit_counts_wrapped_visual_lines() {
    let lines = vec![Line::from("12345678901234567890")];

    assert_eq!(transcript_max_scroll(&lines, 5, 3), 1);
}
