use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use codex_helper_core::codex_switch::CodexSwitchIntent;
use codex_helper_core::config::CodexClientPreset;

use super::{KeyEventContext, handle_key_event};
use crate::dashboard_core::{
    OperatorActionCapabilities, OperatorReadIssue, OperatorReadModel, OperatorReadStatus,
    OperatorRouteCandidateSummary, OperatorRouteTargetSummary, OperatorRoutingSummary,
};
use crate::proxy::{OperatorEndpointMode, OperatorRoutingCommand, OperatorSessionAffinityCommand};
use crate::state::SessionObservationScope;
use crate::tui::Language;
use crate::tui::input::normal::{
    accepts_codex_switch_key, codex_client_preset_for_key, codex_switch_intent_for_key,
};
use crate::tui::model::{ProviderOption, SessionRouteAffinityView, SessionRow, Snapshot};
use crate::tui::operator_actions::PendingOperatorAction;
use crate::tui::state::{RuntimeConnectionKind, UiState};
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

fn routing_snapshot() -> Snapshot {
    Snapshot {
        routing: Some(OperatorRoutingSummary {
            route_graph_key: "routing:sha256:test".to_string(),
            control_revision: 7,
            provider_policy_revision: 11,
            entry: "main".to_string(),
            entry_strategy: crate::config::RouteStrategy::RoundRobin,
            entry_target: None,
            new_session_preference: None,
            affinity_policy: crate::config::RouteAffinityPolicy::FallbackSticky,
            scheduling_preset: crate::config::SchedulingPreset::Balanced,
            fallback_ttl_ms: Some(60_000),
            reprobe_preferred_after_ms: Some(5_000),
            candidates: vec![OperatorRouteCandidateSummary {
                route_order: 0,
                provider_id: "input".to_string(),
                endpoint_id: "primary".to_string(),
                preference_group: 0,
                route_path: vec!["main".to_string()],
            }],
        }),
        ..Snapshot::default()
    }
}

fn affinity_session_row(active_count: usize, revision: Option<&str>) -> SessionRow {
    SessionRow {
        session_id: Some("session:sha256:test".to_string()),
        local_session_id: None,
        observation_scope: SessionObservationScope::ObservedOnly,
        host_local_transcript_path: None,
        last_client_name: None,
        last_client_addr: None,
        cwd: None,
        active_count,
        active_started_at_ms_min: None,
        active_last_method: None,
        active_last_path: None,
        last_status: Some(200),
        last_duration_ms: None,
        last_ended_at_ms: None,
        last_model: None,
        last_reasoning_effort: None,
        last_service_tier: None,
        last_provider_id: Some("input".to_string()),
        last_usage: None,
        total_usage: None,
        turns_total: None,
        turns_with_usage: None,
        last_output_tokens_per_second: None,
        avg_output_tokens_per_second: None,
        binding_profile_name: None,
        binding_continuity_mode: None,
        last_route_decision: None,
        route_affinity: revision.map(|revision| SessionRouteAffinityView {
            revision: revision.to_string(),
            provider_id: "input".to_string(),
            endpoint_id: "primary".to_string(),
            upstream_origin: "https://input.example.test".to_string(),
            route_path: vec!["main".to_string()],
            last_selected_at_ms: 1,
            last_changed_at_ms: 1,
            change_reason: "selected".to_string(),
        }),
        effective_model: None,
        effective_reasoning_effort: None,
        effective_service_tier: None,
    }
}

fn affinity_snapshot(active_count: usize, revision: Option<&str>) -> Snapshot {
    let mut snapshot = routing_snapshot();
    snapshot
        .routing
        .as_mut()
        .expect("routing")
        .candidates
        .push(OperatorRouteCandidateSummary {
            route_order: 1,
            provider_id: "ciii".to_string(),
            endpoint_id: "secondary".to_string(),
            preference_group: 0,
            route_path: vec!["main".to_string()],
        });
    snapshot
        .rows
        .push(affinity_session_row(active_count, revision));
    snapshot
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
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::RefreshBalances { force: false })
    ));
}

#[tokio::test]
async fn routing_g_queues_a_forced_balance_refresh() {
    let mut ui = UiState {
        page: Page::Routing,
        ..UiState::default()
    };
    let snapshot = routing_snapshot();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('g')).await);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::RefreshBalances { force: true })
    ));
}

#[tokio::test]
async fn routing_enter_requires_action_and_confirmation_before_setting_preference() {
    let mut ui = UiState {
        page: Page::Routing,
        ..UiState::default()
    };
    let snapshot = routing_snapshot();

    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::RoutingActions);
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::RoutingConfirmation);
    assert!(ui.pending_operator_action.is_none());
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    let Some(PendingOperatorAction::MutateRouting(request)) = ui.pending_operator_action.as_ref()
    else {
        panic!("new-session preference was not queued");
    };
    assert_eq!(request.expected_route_graph_key, "routing:sha256:test");
    assert_eq!(request.expected_control_revision, 7);
    assert_eq!(request.expected_policy_revision, 11);
    assert_eq!(
        request.command,
        OperatorRoutingCommand::SetNewSessionPreference {
            provider_id: "input".to_string(),
            endpoint_id: "primary".to_string(),
        }
    );
}

#[tokio::test]
async fn routing_backspace_requires_confirmation_before_clearing_with_revision_cas() {
    let mut ui = UiState {
        page: Page::Routing,
        ..UiState::default()
    };
    let mut snapshot = routing_snapshot();
    let routing = snapshot.routing.as_mut().expect("routing");
    routing.new_session_preference = Some(OperatorRouteTargetSummary {
        provider_id: "input".to_string(),
        endpoint_id: "primary".to_string(),
    });

    assert!(press(&mut ui, &snapshot, KeyCode::Backspace).await);
    assert_eq!(ui.overlay, Overlay::RoutingConfirmation);
    assert!(ui.pending_operator_action.is_none());
    assert!(press(&mut ui, &snapshot, KeyCode::Char('y')).await);
    let Some(PendingOperatorAction::MutateRouting(request)) = ui.pending_operator_action.as_ref()
    else {
        panic!("new-session preference clear was not queued");
    };
    assert_eq!(request.expected_route_graph_key, "routing:sha256:test");
    assert_eq!(request.expected_control_revision, 7);
    assert_eq!(request.expected_policy_revision, 11);
    assert_eq!(
        request.command,
        OperatorRoutingCommand::ClearNewSessionPreference
    );
}

#[tokio::test]
async fn routing_a_is_an_easy_to_discover_restore_automatic_alias() {
    let mut ui = UiState {
        page: Page::Routing,
        ..UiState::default()
    };
    let mut snapshot = routing_snapshot();
    let routing = snapshot.routing.as_mut().expect("routing");
    routing.new_session_preference = Some(OperatorRouteTargetSummary {
        provider_id: "input".to_string(),
        endpoint_id: "primary".to_string(),
    });

    assert!(press(&mut ui, &snapshot, KeyCode::Char('a')).await);
    assert_eq!(ui.overlay, Overlay::RoutingConfirmation);
    assert!(matches!(
        ui.routing_confirmation
            .as_ref()
            .map(|request| &request.command),
        Some(OperatorRoutingCommand::ClearNewSessionPreference)
    ));
}

#[tokio::test]
async fn routing_mode_menu_maps_all_endpoint_modes_with_policy_revision_cas() {
    let snapshot = routing_snapshot();
    let cases = [
        (0, OperatorEndpointMode::Enabled),
        (1, OperatorEndpointMode::Draining),
        (2, OperatorEndpointMode::Disabled),
    ];

    for (down_presses, expected_mode) in cases {
        let mut ui = UiState {
            page: Page::Routing,
            ..UiState::default()
        };

        assert!(press(&mut ui, &snapshot, KeyCode::Char('m')).await);
        assert_eq!(ui.overlay, Overlay::RoutingActions);
        assert_eq!(ui.routing_action_selected_idx, 2);
        for _ in 0..down_presses {
            assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
        }
        assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
        let request = ui
            .routing_confirmation
            .as_ref()
            .expect("endpoint mode confirmation");
        assert_eq!(request.expected_route_graph_key, "routing:sha256:test");
        assert_eq!(request.expected_control_revision, 7);
        assert_eq!(request.expected_policy_revision, 11);
        assert_eq!(
            request.command,
            OperatorRoutingCommand::SetEndpointMode {
                provider_id: "input".to_string(),
                endpoint_id: "primary".to_string(),
                mode: expected_mode,
            }
        );
    }
}

#[tokio::test]
async fn routing_confirmation_can_be_cancelled_without_mutation() {
    let mut ui = UiState {
        page: Page::Routing,
        ..UiState::default()
    };
    let snapshot = routing_snapshot();

    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::RoutingActions);
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::RoutingConfirmation);
    assert!(press(&mut ui, &snapshot, KeyCode::Esc).await);

    assert_eq!(ui.overlay, Overlay::None);
    assert!(ui.routing_confirmation.is_none());
    assert!(ui.pending_operator_action.is_none());
}

#[tokio::test]
async fn routing_long_list_supports_page_edges_and_preference_location() {
    let mut snapshot = routing_snapshot();
    let routing = snapshot.routing.as_mut().expect("routing");
    routing.candidates = (0..25)
        .map(|index| OperatorRouteCandidateSummary {
            route_order: index,
            provider_id: format!("provider-{index}"),
            endpoint_id: "default".to_string(),
            preference_group: (index / 5) as u32,
            route_path: vec!["main".to_string()],
        })
        .collect();
    routing.new_session_preference = Some(OperatorRouteTargetSummary {
        provider_id: "provider-17".to_string(),
        endpoint_id: "default".to_string(),
    });
    let mut ui = UiState {
        page: Page::Routing,
        routing_candidates_visible_rows: 5,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.selected_routing_candidate_idx, 5);
    assert!(press(&mut ui, &snapshot, KeyCode::End).await);
    assert_eq!(ui.selected_routing_candidate_idx, 24);
    assert!(press(&mut ui, &snapshot, KeyCode::PageUp).await);
    assert_eq!(ui.selected_routing_candidate_idx, 19);
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert_eq!(ui.selected_routing_candidate_idx, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('p')).await);
    assert_eq!(ui.selected_routing_candidate_idx, 17);
}

#[tokio::test]
async fn remote_routing_actions_are_handled_as_read_only() {
    let mut ui = UiState {
        page: Page::Routing,
        runtime_connection: RuntimeConnectionKind::RemoteObserver,
        ..UiState::default()
    };
    let snapshot = routing_snapshot();

    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert!(ui.pending_operator_action.is_none());
    assert!(
        ui.toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("read-only"))
    );
}

#[tokio::test]
async fn remote_routing_rejects_every_mutating_and_refresh_shortcut() {
    let mut snapshot = routing_snapshot();
    snapshot
        .routing
        .as_mut()
        .expect("routing")
        .new_session_preference = Some(OperatorRouteTargetSummary {
        provider_id: "input".to_string(),
        endpoint_id: "primary".to_string(),
    });

    for code in [
        KeyCode::Char('g'),
        KeyCode::Char('a'),
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Char('m'),
    ] {
        let mut ui = UiState {
            page: Page::Routing,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..UiState::default()
        };

        assert!(press(&mut ui, &snapshot, code).await, "unhandled {code:?}");
        assert_eq!(ui.overlay, Overlay::None, "unexpected overlay for {code:?}");
        assert!(
            ui.routing_confirmation.is_none(),
            "confirmation for {code:?}"
        );
        assert!(
            ui.pending_operator_action.is_none(),
            "queued action for {code:?}"
        );
        assert!(
            ui.toast
                .as_ref()
                .is_some_and(|(message, _)| message.contains("read-only")),
            "missing read-only notice for {code:?}"
        );
    }
}

#[tokio::test]
async fn session_affinity_actions_require_an_idle_bound_session_with_revision() {
    let cases = vec![
        (
            affinity_snapshot(1, Some("affinity:v1:current")),
            "active request",
        ),
        (affinity_snapshot(0, None), "no affinity"),
        (affinity_snapshot(0, Some("")), "read-only mode"),
    ];

    for (snapshot, expected_message) in cases {
        let mut ui = UiState {
            page: Page::Sessions,
            ..UiState::default()
        };

        assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
        assert_eq!(ui.overlay, Overlay::None);
        assert!(ui.pending_operator_action.is_none());
        assert!(
            ui.toast
                .as_ref()
                .is_some_and(|(message, _)| message.contains(expected_message)),
            "missing {expected_message:?}: {:?}",
            ui.toast
        );
    }
}

#[tokio::test]
async fn session_affinity_rebind_uses_affinity_revision_cas() {
    let snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    let mut ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::SessionAffinityActions);
    assert_eq!(ui.session_affinity_action_selected_idx, 1);
    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::SessionAffinityConfirmation);
    let confirmation = ui
        .session_affinity_confirmation
        .as_ref()
        .expect("session affinity confirmation")
        .clone();
    assert_eq!(confirmation.session_key, "session:sha256:test");
    assert_eq!(
        confirmation.expected_affinity_revision.as_deref(),
        Some("affinity:v1:current")
    );
    assert_eq!(
        confirmation.command,
        OperatorSessionAffinityCommand::Rebind {
            provider_id: "ciii".to_string(),
            endpoint_id: "secondary".to_string(),
        }
    );
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionAffinity(ref request))
            if request == &confirmation
    ));
}

#[tokio::test]
async fn conditional_session_affinity_actions_offer_clear_only() {
    let mut snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    snapshot.routing.as_mut().expect("routing").entry_strategy =
        crate::config::RouteStrategy::Conditional;
    let mut ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::SessionAffinityActions);
    assert_eq!(ui.session_affinity_action_selected_idx, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.session_affinity_action_selected_idx, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::SessionAffinityConfirmation);
    assert_eq!(
        ui.session_affinity_confirmation
            .as_ref()
            .map(|request| &request.command),
        Some(&OperatorSessionAffinityCommand::Clear)
    );
}

#[tokio::test]
async fn session_affinity_clear_is_explicit_and_confirmation_gated() {
    let snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    let mut ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    let confirmation = ui
        .session_affinity_confirmation
        .as_ref()
        .expect("clear confirmation");
    assert_eq!(confirmation.command, OperatorSessionAffinityCommand::Clear);
    assert!(ui.pending_operator_action.is_none());

    assert!(press(&mut ui, &snapshot, KeyCode::Char('y')).await);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionAffinity(ref request))
            if request.command == OperatorSessionAffinityCommand::Clear
    ));
}

#[tokio::test]
async fn session_affinity_capability_downgrades_are_read_only() {
    let snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    let stale_read_model = OperatorReadModel {
        api_version: 1,
        service_name: "codex".to_string(),
        status: OperatorReadStatus::Stale,
        captured_at_ms: 1,
        revisions: None,
        data: None,
        issue: Some(OperatorReadIssue::RefreshFailed),
    };
    let cases = [
        UiState {
            page: Page::Sessions,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..UiState::default()
        },
        UiState {
            page: Page::Sessions,
            operator_read_model: Some(stale_read_model),
            ..UiState::default()
        },
        UiState {
            page: Page::Sessions,
            runtime_connection: RuntimeConnectionKind::LocalAttached,
            local_operator_transport_available: true,
            operator_action_capabilities: OperatorActionCapabilities {
                refresh_provider_balances: true,
                mutate_routing: true,
                mutate_session_affinity: false,
            },
            ..UiState::default()
        },
    ];

    for mut ui in cases {
        assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
        assert_eq!(ui.overlay, Overlay::None);
        assert!(ui.pending_operator_action.is_none());
        assert!(
            ui.toast
                .as_ref()
                .is_some_and(|(message, _)| message.contains("read-only")),
            "{:?}",
            ui.toast
        );
    }

    let mut writable_attached = UiState {
        page: Page::Sessions,
        runtime_connection: RuntimeConnectionKind::LocalAttached,
        local_operator_transport_available: true,
        operator_action_capabilities: OperatorActionCapabilities {
            refresh_provider_balances: false,
            mutate_routing: false,
            mutate_session_affinity: true,
        },
        ..UiState::default()
    };
    assert!(press(&mut writable_attached, &snapshot, KeyCode::Enter).await);
    assert_eq!(writable_attached.overlay, Overlay::SessionAffinityActions);
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
        (Page::Routing, vec!['r', 'h', 'H', 'c', 'C', 'o', 'O']),
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

#[test]
fn settings_preset_keys_cover_the_full_v0203_preset_set() {
    for (key, preset) in [
        ('B', CodexClientPreset::ChatGptBridge),
        ('I', CodexClientPreset::ImagegenBridge),
        ('F', CodexClientPreset::OfficialRelay),
        ('V', CodexClientPreset::OfficialImagegen),
        ('D', CodexClientPreset::Default),
    ] {
        assert_eq!(
            codex_client_preset_for_key(KeyCode::Char(key)),
            Some(preset)
        );
    }
    assert_eq!(codex_client_preset_for_key(KeyCode::Char('b')), None);
}

#[test]
fn settings_switch_actions_ignore_key_repeat_events() {
    let pressed =
        KeyEvent::new_with_kind(KeyCode::Char('V'), KeyModifiers::NONE, KeyEventKind::Press);
    let repeated =
        KeyEvent::new_with_kind(KeyCode::Char('V'), KeyModifiers::NONE, KeyEventKind::Repeat);

    assert!(accepts_codex_switch_key(&pressed));
    assert!(!accepts_codex_switch_key(&repeated));
}
