use super::{
    current_page_help_lines, help_quit_line_for_tests, help_text_for_tests, transcript_max_scroll,
};
use crate::tui::Language;
use crate::tui::model::Palette;
use crate::tui::types::Page;
use ratatui::prelude::Line;

#[test]
fn routing_help_only_advertises_read_only_inspection() {
    let lines =
        current_page_help_lines(Language::En, Page::Stations, true, true, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("Current page: Routing"), "{text}");
    assert!(text.contains("read-only provider details"), "{text}");
    for removed in [
        "refresh balances",
        "Backspace",
        "route target",
        "health check",
        "editor",
    ] {
        assert!(!text.contains(removed), "unexpected {removed:?} in {text}");
    }
}

#[test]
fn usage_help_keeps_navigation_and_report_export() {
    let lines = current_page_help_lines(Language::En, Page::Stats, true, true, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("endpoint / provider day ranking"), "{text}");
    assert!(text.contains("move the active ranking selection"), "{text}");
    assert!(text.contains("export and copy"), "{text}");
}

#[test]
fn settings_help_only_advertises_the_local_codex_switch_action() {
    let lines =
        current_page_help_lines(Language::En, Page::Settings, true, true, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("n/o"), "{text}");
    assert!(text.contains("switch"), "{text}");
    for removed in ["reload", "diagnos", "smoke", "manage profile", "patch"] {
        assert!(!text.contains(removed), "unexpected {removed:?} in {text}");
    }
}

#[test]
fn help_quit_line_never_promises_runtime_shutdown() {
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
    assert!(
        integrated.contains("keep the runtime running"),
        "{integrated}"
    );
    assert!(!attached.contains("shutdown"), "{attached}");
    assert!(!integrated.contains("shutdown"), "{integrated}");
}

#[test]
fn transcript_scroll_limit_counts_wrapped_visual_lines() {
    let lines = vec![Line::from("12345678901234567890")];

    assert_eq!(transcript_max_scroll(&lines, 5, 3), 1);
}
