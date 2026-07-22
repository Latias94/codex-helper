use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use codex_helper_core::codex_switch::CodexSwitchIntent;
use codex_helper_core::config::CodexClientPreset;
use codex_helper_core::service_status::{
    ServiceStatusKind, ServiceStatusProbeSnapshot, ServiceStatusServiceSnapshot,
    ServiceStatusSnapshot,
};

use super::{KeyEventContext, handle_key_event, persist_host_local_language_change_with};
use crate::dashboard_core::{
    ControlProfileOption, OperatorActionCapabilities, OperatorReadIssue, OperatorReadModel,
    OperatorReadStatus, OperatorRouteCandidateSummary, OperatorRouteTargetSummary,
    OperatorRoutingSummary,
};
use crate::proxy::{
    CODEX_RELAY_LIVE_SMOKE_ACK, CodexRelayLiveSmokeCase, OperatorEndpointMode,
    OperatorRoutingCommand, OperatorSessionAffinityCommand, OperatorSessionBindingCommand,
};
use crate::state::SessionObservationScope;
use crate::tui::Language;
use crate::tui::input::normal::{
    accepts_codex_switch_key, codex_client_preset_for_key, codex_switch_intent_for_key,
};
use crate::tui::model::{ProviderOption, SessionRouteAffinityView, SessionRow, Snapshot};
use crate::tui::operator_actions::PendingOperatorAction;
use crate::tui::state::{RecentCodexRow, RuntimeConnectionKind, UiState};
use crate::tui::types::{Focus, Overlay, Page, StatsFocus};

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

fn service_status_input_snapshot() -> Snapshot {
    Snapshot {
        service_status: Some(ServiceStatusSnapshot {
            generated_at_ms: 1,
            configured: true,
            enabled: true,
            refresh_interval_secs: 60,
            history_cells: 60,
            probes: (0..3)
                .map(|index| ServiceStatusProbeSnapshot {
                    id: format!("provider-{index}"),
                    url: format!("https://provider-{index}.example"),
                    fetched_at_ms: 1,
                    generated_at_ms: None,
                    all_ok: Some(true),
                    services: vec![ServiceStatusServiceSnapshot {
                        model: format!("model-{index}"),
                        uptime_pct: Some("100%".to_string()),
                        latest_kind: ServiceStatusKind::Ok,
                        latest: None,
                        history: Vec::new(),
                    }],
                    credential_readiness: None,
                    credential_details: Vec::new(),
                    error: None,
                })
                .collect(),
            error: None,
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
        binding: crate::state::SessionBindingProjection {
            revision: "binding:v1:none".to_string(),
            ..crate::state::SessionBindingProjection::default()
        },
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
        .rows
        .push(affinity_session_row(active_count, revision));
    snapshot
}

const BRIDGE_LOCAL_SESSION_ID: &str = "local-session-bridge-test";

fn bridge_snapshot() -> Snapshot {
    let mut snapshot = affinity_snapshot(0, None);
    snapshot.rows[0].local_session_id = Some(BRIDGE_LOCAL_SESSION_ID.to_string());
    snapshot
}

fn bridge_history_ui(runtime_connection: RuntimeConnectionKind) -> UiState {
    UiState {
        page: Page::History,
        language: Language::En,
        runtime_connection,
        local_operator_transport_available: true,
        codex_history_sessions: vec![crate::sessions::SessionSummary {
            id: BRIDGE_LOCAL_SESSION_ID.to_string(),
            path: "bridge-session.jsonl".into(),
            cwd: None,
            created_at: None,
            updated_at: None,
            last_response_at: None,
            user_turns: 0,
            assistant_turns: 0,
            rounds: 0,
            first_user_message: None,
            source: crate::sessions::SessionSummarySource::LocalFile,
            sort_hint_ms: None,
        }],
        ..UiState::default()
    }
}

fn bridge_recent_ui(runtime_connection: RuntimeConnectionKind) -> UiState {
    UiState {
        page: Page::Recent,
        language: Language::En,
        runtime_connection,
        local_operator_transport_available: true,
        codex_recent_rows: vec![RecentCodexRow {
            root: "/workspace/project".to_string(),
            branch: Some("main".to_string()),
            session_id: BRIDGE_LOCAL_SESSION_ID.to_string(),
            cwd: Some("/workspace/project".to_string()),
            mtime_ms: crate::tui::model::now_ms(),
        }],
        ..UiState::default()
    }
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
async fn entering_recent_preserves_the_selected_session_identity() {
    let now = crate::tui::model::now_ms();
    let row = |session_id: &str| RecentCodexRow {
        root: format!("/{session_id}"),
        branch: None,
        session_id: session_id.to_string(),
        cwd: None,
        mtime_ms: now,
    };
    let mut ui = UiState {
        codex_recent_rows: vec![row("session-a"), row("session-b")],
        codex_recent_selected_idx: 1,
        codex_recent_selected_id: Some("session-b".to_string()),
        ..UiState::default()
    };
    ui.codex_recent_table.select(Some(1));

    assert!(press(&mut ui, &Snapshot::default(), KeyCode::Char('9')).await);

    assert_eq!(ui.page, Page::Recent);
    assert_eq!(ui.codex_recent_selected_idx, 1);
    assert_eq!(ui.codex_recent_selected_id.as_deref(), Some("session-b"));
    assert_eq!(ui.codex_recent_table.selected(), Some(1));
}

#[tokio::test]
async fn changing_recent_window_preserves_a_still_visible_session() {
    let now = crate::tui::model::now_ms();
    let row = |session_id: &str| RecentCodexRow {
        root: format!("/{session_id}"),
        branch: None,
        session_id: session_id.to_string(),
        cwd: None,
        mtime_ms: now,
    };
    let mut ui = UiState {
        page: Page::Recent,
        codex_recent_rows: vec![row("session-a"), row("session-b")],
        codex_recent_selected_idx: 1,
        codex_recent_selected_id: Some("session-b".to_string()),
        ..UiState::default()
    };
    ui.codex_recent_table.select(Some(1));

    assert!(press(&mut ui, &Snapshot::default(), KeyCode::Char(']')).await);

    assert_eq!(ui.codex_recent_selected_idx, 1);
    assert_eq!(ui.codex_recent_selected_id.as_deref(), Some("session-b"));
    assert_eq!(ui.codex_recent_table.selected(), Some(1));
}

#[tokio::test]
async fn remote_observer_uses_shared_history_recent_and_filter_controls() {
    let snapshot = Snapshot::default();
    let mut ui = UiState {
        runtime_connection: RuntimeConnectionKind::RemoteObserver,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Char('8')).await);
    assert_eq!(ui.page, Page::History);
    assert!(ui.needs_codex_history_refresh);

    ui.needs_codex_history_refresh = false;
    assert!(press(&mut ui, &snapshot, KeyCode::Char('9')).await);
    assert_eq!(ui.page, Page::Recent);
    assert!(ui.needs_codex_recent_refresh);

    ui.page = Page::Sessions;
    assert!(press(&mut ui, &snapshot, KeyCode::Char('a')).await);
    assert!(ui.sessions_page_active_only);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('e')).await);
    assert!(ui.sessions_page_errors_only);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('r')).await);
    assert!(!ui.sessions_page_active_only);
    assert!(!ui.sessions_page_errors_only);

    ui.page = Page::Requests;
    assert!(press(&mut ui, &snapshot, KeyCode::Char('e')).await);
    assert!(ui.request_page_errors_only);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('c')).await);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('s')).await);
    assert!(ui.request_page_scope_session);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('x')).await);
    assert!(ui.focused_request_session_id.is_none());
}

#[tokio::test]
async fn remote_observer_history_navigation_uses_the_history_table() {
    let snapshot = Snapshot::default();
    let mut ui = UiState {
        page: Page::History,
        runtime_connection: RuntimeConnectionKind::RemoteObserver,
        codex_history_sessions: vec![
            crate::sessions::SessionSummary {
                id: "session-a".to_string(),
                path: "a.jsonl".into(),
                cwd: None,
                created_at: None,
                updated_at: None,
                last_response_at: None,
                user_turns: 0,
                assistant_turns: 0,
                rounds: 0,
                first_user_message: None,
                source: crate::sessions::SessionSummarySource::LocalFile,
                sort_hint_ms: None,
            },
            crate::sessions::SessionSummary {
                id: "session-b".to_string(),
                path: "b.jsonl".into(),
                cwd: None,
                created_at: None,
                updated_at: None,
                last_response_at: None,
                user_turns: 0,
                assistant_turns: 0,
                rounds: 0,
                first_user_message: None,
                source: crate::sessions::SessionSummarySource::LocalFile,
                sort_hint_ms: None,
            },
        ],
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.selected_codex_history_idx, 1);
    assert_eq!(ui.selected_codex_history_id.as_deref(), Some("session-b"));
    assert_eq!(ui.codex_history_table.selected(), Some(1));
    assert_eq!(ui.sessions_table.selected(), None);
}

#[tokio::test]
async fn history_navigation_reaches_the_loader_tail_after_external_focus_insertion() {
    let summary = |index: usize| crate::sessions::SessionSummary {
        id: format!("session-{index}"),
        path: format!("{index}.jsonl").into(),
        cwd: None,
        created_at: None,
        updated_at: None,
        last_response_at: None,
        user_turns: 0,
        assistant_turns: 0,
        rounds: 0,
        first_user_message: None,
        source: crate::sessions::SessionSummarySource::LocalFile,
        sort_hint_ms: None,
    };
    let mut ui = UiState {
        page: Page::History,
        codex_history_sessions: (0..301).map(summary).collect(),
        selected_codex_history_idx: 299,
        selected_codex_history_id: Some("session-299".to_string()),
        ..UiState::default()
    };
    ui.codex_history_table.select(Some(299));

    assert!(press(&mut ui, &Snapshot::default(), KeyCode::Down).await);

    assert_eq!(ui.selected_codex_history_idx, 300);
    assert_eq!(ui.selected_codex_history_id.as_deref(), Some("session-300"));
    assert_eq!(ui.codex_history_table.selected(), Some(300));
}

#[tokio::test]
async fn remote_observer_history_bridge_shortcuts_stay_on_history() {
    let snapshot = bridge_snapshot();

    for code in ['s', 'f'] {
        let mut ui = bridge_history_ui(RuntimeConnectionKind::RemoteObserver);

        assert!(press(&mut ui, &snapshot, KeyCode::Char(code)).await);
        assert_eq!(ui.page, Page::History, "key={code}");
        assert!(ui.focused_request_session_id.is_none(), "key={code}");
        assert!(
            ui.toast.as_ref().is_some_and(|(message, _)| {
                message.contains("remote observer")
                    && message.contains("observer-local")
                    && message.contains("Sessions/Requests")
            }),
            "key={code}, toast={:?}",
            ui.toast
        );
    }
}

#[tokio::test]
async fn remote_observer_recent_bridge_shortcuts_stay_on_recent() {
    let snapshot = bridge_snapshot();

    for code in ['s', 'f'] {
        let mut ui = bridge_recent_ui(RuntimeConnectionKind::RemoteObserver);

        assert!(press(&mut ui, &snapshot, KeyCode::Char(code)).await);
        assert_eq!(ui.page, Page::Recent, "key={code}");
        assert!(ui.focused_request_session_id.is_none(), "key={code}");
        assert!(
            ui.toast.as_ref().is_some_and(|(message, _)| {
                message.contains("remote observer")
                    && message.contains("observer-local")
                    && message.contains("Sessions/Requests")
            }),
            "key={code}, toast={:?}",
            ui.toast
        );
    }
}

#[tokio::test]
async fn local_history_and_recent_bridge_shortcuts_keep_their_navigation() {
    let snapshot = bridge_snapshot();

    for runtime_connection in [
        RuntimeConnectionKind::Integrated,
        RuntimeConnectionKind::LocalAttached,
    ] {
        for mut ui in [
            bridge_history_ui(runtime_connection),
            bridge_recent_ui(runtime_connection),
        ] {
            assert!(press(&mut ui, &snapshot, KeyCode::Char('s')).await);
            assert_eq!(ui.page, Page::Sessions, "mode={runtime_connection:?}");
            assert_eq!(ui.selected_session_idx, 0, "mode={runtime_connection:?}");
        }

        for mut ui in [
            bridge_history_ui(runtime_connection),
            bridge_recent_ui(runtime_connection),
        ] {
            assert!(press(&mut ui, &snapshot, KeyCode::Char('f')).await);
            assert_eq!(ui.page, Page::Requests, "mode={runtime_connection:?}");
            assert_eq!(
                ui.focused_request_session_id.as_deref(),
                Some("session:sha256:test"),
                "mode={runtime_connection:?}"
            );
        }
    }
}

#[tokio::test]
async fn history_and_recent_detail_scroll_keys_use_independent_offsets() {
    let snapshot = Snapshot::default();
    let mut ui = UiState {
        page: Page::History,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.codex_history_details_scroll, 8);
    assert!(press(&mut ui, &snapshot, KeyCode::End).await);
    assert_eq!(ui.codex_history_details_scroll, u16::MAX);
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert_eq!(ui.codex_history_details_scroll, 0);

    ui.page = Page::Recent;
    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.codex_recent_details_scroll, 8);
    assert_eq!(ui.codex_history_details_scroll, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::PageUp).await);
    assert_eq!(ui.codex_recent_details_scroll, 0);
}

#[tokio::test]
async fn dashboard_page_keys_scroll_session_details_without_changing_focus() {
    let snapshot = Snapshot::default();
    let mut ui = UiState {
        page: Page::Dashboard,
        focus: Focus::Requests,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.dashboard_details_scroll, 8);
    assert_eq!(ui.focus, Focus::Requests);
    assert!(press(&mut ui, &snapshot, KeyCode::End).await);
    assert_eq!(ui.dashboard_details_scroll, u16::MAX);
    assert!(press(&mut ui, &snapshot, KeyCode::PageUp).await);
    assert_eq!(ui.dashboard_details_scroll, u16::MAX - 8);
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert_eq!(ui.dashboard_details_scroll, 0);
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
async fn routing_detail_focus_scrolls_without_moving_the_selected_candidate() {
    let mut ui = UiState {
        page: Page::Routing,
        selected_routing_candidate_idx: 0,
        routing_detail_available: true,
        ..UiState::default()
    };
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

    assert!(press(&mut ui, &snapshot, KeyCode::Tab).await);
    assert!(ui.routing_detail_focused);
    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.routing_detail_scroll, 1);
    assert_eq!(ui.selected_routing_candidate_idx, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.routing_detail_scroll, 9);
    assert!(press(&mut ui, &snapshot, KeyCode::End).await);
    assert_eq!(ui.routing_detail_scroll, u16::MAX);
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert_eq!(ui.routing_detail_scroll, 0);

    assert!(press(&mut ui, &snapshot, KeyCode::Tab).await);
    assert!(!ui.routing_detail_focused);
    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.selected_routing_candidate_idx, 1);
    assert_eq!(ui.routing_detail_scroll, 0);
}

#[tokio::test]
async fn service_status_keys_move_rows_and_scroll_focused_details() {
    let snapshot = service_status_input_snapshot();
    let mut ui = UiState {
        page: Page::ServiceStatus,
        service_status_visible_rows: 2,
        service_status_detail_available: true,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::End).await);
    assert_eq!(ui.selected_service_status_idx, 2);
    assert_eq!(
        ui.selected_service_status_key,
        Some(("provider-2".to_string(), Some("model-2".to_string())))
    );
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert_eq!(ui.selected_service_status_idx, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.selected_service_status_idx, 2);

    assert!(press(&mut ui, &snapshot, KeyCode::Tab).await);
    assert!(ui.service_status_detail_focused);
    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.service_status_detail_scroll, 1);
    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.service_status_detail_scroll, 9);
    assert!(press(&mut ui, &snapshot, KeyCode::End).await);
    assert_eq!(ui.service_status_detail_scroll, u16::MAX);
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert_eq!(ui.service_status_detail_scroll, 0);

    assert!(press(&mut ui, &snapshot, KeyCode::Tab).await);
    assert!(!ui.service_status_detail_focused);
    assert!(press(&mut ui, &snapshot, KeyCode::Char('r')).await);
    assert!(ui.needs_snapshot_refresh);
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
async fn session_affinity_actions_require_an_idle_bound_session_with_revision_for_rebind() {
    let cases = vec![
        (
            affinity_snapshot(1, Some("affinity:v1:current")),
            "active request",
        ),
        (affinity_snapshot(0, Some("")), "read-only mode"),
    ];

    for (snapshot, expected_message) in cases {
        let mut ui = UiState {
            page: Page::Sessions,
            ..UiState::default()
        };

        assert!(press(&mut ui, &snapshot, KeyCode::Char('A')).await);
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
async fn session_affinity_bind_initializes_an_idle_unbound_session_without_a_revision() {
    let snapshot = affinity_snapshot(0, None);
    let mut ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Char('p')).await);
    assert_eq!(ui.overlay, Overlay::SessionAffinityActions);
    assert_eq!(ui.session_affinity_action_selected_idx, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(ui.overlay, Overlay::SessionAffinityConfirmation);
    let confirmation = ui
        .session_affinity_confirmation
        .as_ref()
        .expect("session affinity bind confirmation")
        .clone();
    assert_eq!(confirmation.session_key, "session:sha256:test");
    assert!(confirmation.expected_affinity_revision.is_none());
    assert_eq!(
        confirmation.command,
        OperatorSessionAffinityCommand::Bind {
            provider_id: "input".to_string(),
            endpoint_id: "primary".to_string(),
        }
    );
    assert!(press(&mut ui, &snapshot, KeyCode::Char('y')).await);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionAffinity(ref request))
            if request == &confirmation
    ));
}

#[tokio::test]
async fn session_affinity_rebind_uses_affinity_revision_cas() {
    let mut snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
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
    let mut ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Char('A')).await);
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

    assert!(press(&mut ui, &snapshot, KeyCode::Char('A')).await);
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

    assert!(press(&mut ui, &snapshot, KeyCode::Char('A')).await);
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
                mutate_session_binding: false,
                reload_runtime: false,
                mutate_default_profile: false,
                inspect_relay_capabilities: false,
                run_relay_live_smoke: false,
            },
            ..UiState::default()
        },
    ];

    for mut ui in cases {
        assert!(press(&mut ui, &snapshot, KeyCode::Char('A')).await);
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
            mutate_session_binding: false,
            reload_runtime: false,
            mutate_default_profile: false,
            inspect_relay_capabilities: false,
            run_relay_live_smoke: false,
        },
        ..UiState::default()
    };
    assert!(press(&mut writable_attached, &snapshot, KeyCode::Char('A')).await);
    assert_eq!(writable_attached.overlay, Overlay::SessionAffinityActions);
}

#[tokio::test]
async fn stats_refresh_requests_balances_and_a_new_operator_read_model() {
    let mut ui = UiState {
        page: Page::Stats,
        ..UiState::default()
    };
    let snapshot = Snapshot::default();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('g')).await);
    assert!(ui.needs_snapshot_refresh);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::RefreshBalances { force: true })
    ));
}

#[tokio::test]
async fn stats_keyboard_selection_follows_provider_keys_after_ranking_reorders() {
    let row = |name: &str| crate::state::UsageDayDimensionRow {
        name: name.to_string(),
        ..crate::state::UsageDayDimensionRow::default()
    };
    let mut snapshot = Snapshot::default();
    snapshot.usage_day.provider_rows = vec![row("provider-a"), row("provider-b")];
    snapshot.usage_day.provider_endpoint_rows =
        vec![row("provider-a/primary"), row("provider-b/primary")];
    let mut ui = UiState {
        page: Page::Stats,
        stats_focus: StatsFocus::Providers,
        ..UiState::default()
    };
    ui.clamp_selection(&snapshot, 0);

    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.selected_stats_provider_idx, 1);
    assert_eq!(
        ui.selected_stats_provider_key.as_deref(),
        Some("provider-b")
    );

    snapshot.usage_day.provider_rows.swap(0, 1);
    ui.clamp_selection(&snapshot, 0);
    assert_eq!(ui.selected_stats_provider_idx, 0);
    assert_eq!(
        ui.selected_stats_provider_key.as_deref(),
        Some("provider-b")
    );

    ui.stats_focus = StatsFocus::ProviderEndpoints;
    ui.clamp_selection(&snapshot, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.selected_stats_provider_endpoint_idx, 1);
    assert_eq!(
        ui.selected_stats_provider_endpoint_key.as_deref(),
        Some("provider-b/primary")
    );

    snapshot.usage_day.provider_endpoint_rows.swap(0, 1);
    ui.clamp_selection(&snapshot, 0);
    assert_eq!(ui.selected_stats_provider_endpoint_idx, 0);
    assert_eq!(
        ui.selected_stats_provider_endpoint_key.as_deref(),
        Some("provider-b/primary")
    );
}

#[tokio::test]
async fn remote_stats_refresh_is_explicitly_snapshot_only() {
    let mut ui = UiState {
        page: Page::Stats,
        runtime_connection: RuntimeConnectionKind::RemoteObserver,
        ..UiState::default()
    };
    let snapshot = Snapshot::default();

    assert!(press(&mut ui, &snapshot, KeyCode::Char('g')).await);
    assert!(ui.needs_snapshot_refresh);
    assert!(ui.pending_operator_action.is_none());
    assert!(
        ui.toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("snapshot-only")),
        "{:?}",
        ui.toast
    );
}

#[tokio::test]
async fn remote_observer_language_toggle_is_scoped_to_the_current_tui_state() {
    let mut ui = UiState {
        language: Language::En,
        runtime_connection: RuntimeConnectionKind::RemoteObserver,
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
async fn integrated_language_change_persists_and_reports_success() {
    let mut ui = UiState {
        language: Language::Zh,
        runtime_connection: RuntimeConnectionKind::Integrated,
        ..UiState::default()
    };
    let mut persisted = None;

    persist_host_local_language_change_with(&mut ui, Language::En, |language| {
        persisted = Some(language);
        async { Ok(()) }
    })
    .await;

    assert_eq!(persisted, Some(Language::Zh));
    assert!(
        ui.toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("已保存")),
        "{:?}",
        ui.toast
    );
}

#[tokio::test]
async fn local_attached_language_change_persists_like_integrated_mode() {
    let mut ui = UiState {
        language: Language::Zh,
        runtime_connection: RuntimeConnectionKind::LocalAttached,
        ..UiState::default()
    };
    let mut persisted = None;

    persist_host_local_language_change_with(&mut ui, Language::En, |language| {
        persisted = Some(language);
        async { Ok(()) }
    })
    .await;

    assert_eq!(persisted, Some(Language::Zh));
    assert!(
        ui.toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("已保存")),
        "{:?}",
        ui.toast
    );
}

#[tokio::test]
async fn integrated_language_save_failure_keeps_selected_language_visible() {
    let mut ui = UiState {
        language: Language::En,
        runtime_connection: RuntimeConnectionKind::Integrated,
        ..UiState::default()
    };

    persist_host_local_language_change_with(&mut ui, Language::Zh, |_| async {
        anyhow::bail!("injected write failure")
    })
    .await;

    assert_eq!(ui.language, Language::En);
    assert!(
        ui.toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("save failed")
                && message.contains("injected write failure")),
        "{:?}",
        ui.toast
    );
}

#[tokio::test]
async fn session_profile_binding_uses_the_visible_revision_and_typed_command() {
    let snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    let mut ui = UiState {
        page: Page::Sessions,
        profile_options: vec![ControlProfileOption {
            name: "daily".to_string(),
            extends: None,
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            fast_mode: true,
            is_default: false,
        }],
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Char('b')).await);
    assert_eq!(ui.overlay, Overlay::SessionProfileMenu);
    assert_eq!(ui.session_profile_menu_idx, 1);
    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.session_key == "session:sha256:test"
                && request.expected_binding_revision == "binding:v1:none"
                && request.command == (OperatorSessionBindingCommand::SetProfile {
                    profile_name: Some("daily".to_string())
                })
    ));
}

#[tokio::test]
async fn default_profile_menu_keeps_the_opened_catalog_identity_across_projection_reordering() {
    let snapshot = Snapshot::default();
    let profile = |name: &str, model: &str| ControlProfileOption {
        name: name.to_string(),
        extends: None,
        model: Some(model.to_string()),
        reasoning_effort: Some("high".to_string()),
        service_tier: Some("priority".to_string()),
        fast_mode: true,
        is_default: false,
    };
    let mut ui = UiState {
        page: Page::Settings,
        profile_options: vec![profile("alpha", "gpt-alpha"), profile("beta", "gpt-beta")],
        configured_default_profile: Some("alpha".to_string()),
        effective_default_profile: Some("alpha".to_string()),
        default_profile_control_revision: 7,
        profile_catalog_key: "catalog:opened".to_string(),
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Char('p')).await);
    assert_eq!(ui.settings_profile_menu_idx, 1);

    ui.profile_options.swap(0, 1);
    ui.profile_catalog_key = "catalog:refreshed".to_string();
    ui.default_profile_control_revision = 8;
    ui.configured_default_profile = Some("beta".to_string());
    ui.effective_default_profile = Some("beta".to_string());

    assert!(press(&mut ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::MutateDefaultProfile(ref request))
            if request.profile_name.as_deref() == Some("alpha")
                && request.expected_profile_catalog_key == "catalog:opened"
                && request.expected_control_revision == 7
                && request.expected_configured_profile.as_deref() == Some("alpha")
                && request.expected_runtime_profile.is_none()
    ));
    assert!(ui.profile_menu_snapshot.is_none());
}

#[tokio::test]
async fn session_profile_and_model_menus_keep_opened_names_across_projection_reordering() {
    let snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    let profiles = || {
        ["alpha", "beta"]
            .into_iter()
            .map(|name| ControlProfileOption {
                name: name.to_string(),
                extends: None,
                model: Some(format!("gpt-{name}")),
                reasoning_effort: None,
                service_tier: None,
                fast_mode: false,
                is_default: false,
            })
            .collect::<Vec<_>>()
    };

    let mut profile_ui = UiState {
        page: Page::Sessions,
        profile_options: profiles(),
        profile_catalog_key: "catalog:opened".to_string(),
        ..UiState::default()
    };
    assert!(press(&mut profile_ui, &snapshot, KeyCode::Char('b')).await);
    assert_eq!(profile_ui.session_profile_menu_idx, 1);
    profile_ui.profile_options.swap(0, 1);
    profile_ui.profile_catalog_key = "catalog:refreshed".to_string();
    assert!(press(&mut profile_ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        profile_ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.command == (OperatorSessionBindingCommand::SetProfile {
                profile_name: Some("alpha".to_string())
            })
    ));

    let mut model_ui = UiState {
        page: Page::Sessions,
        profile_options: profiles(),
        profile_catalog_key: "catalog:opened".to_string(),
        ..UiState::default()
    };
    assert!(press(&mut model_ui, &snapshot, KeyCode::Char('M')).await);
    assert!(press(&mut model_ui, &snapshot, KeyCode::Down).await);
    assert_eq!(model_ui.session_model_menu_idx, 1);
    model_ui.profile_options.swap(0, 1);
    model_ui.profile_catalog_key = "catalog:refreshed".to_string();
    assert!(press(&mut model_ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        model_ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.command == (OperatorSessionBindingCommand::SetModel {
                model: Some("gpt-alpha".to_string())
            })
    ));
}

#[tokio::test]
async fn session_model_effort_and_fast_controls_queue_typed_mutations() {
    let snapshot = affinity_snapshot(0, Some("affinity:v1:current"));

    let mut model_ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };
    assert!(press(&mut model_ui, &snapshot, KeyCode::Char('M')).await);
    assert_eq!(model_ui.overlay, Overlay::SessionModelMenu);
    assert!(press(&mut model_ui, &snapshot, KeyCode::End).await);
    assert!(press(&mut model_ui, &snapshot, KeyCode::Enter).await);
    assert_eq!(model_ui.overlay, Overlay::SessionBindingInput);
    for character in "gpt-custom".chars() {
        assert!(press(&mut model_ui, &snapshot, KeyCode::Char(character)).await);
    }
    assert!(press(&mut model_ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        model_ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.command == (OperatorSessionBindingCommand::SetModel {
                model: Some("gpt-custom".to_string())
            })
    ));

    let mut effort_ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };
    assert!(press(&mut effort_ui, &snapshot, KeyCode::Char('E')).await);
    assert!(press(&mut effort_ui, &snapshot, KeyCode::End).await);
    assert!(press(&mut effort_ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        effort_ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.command == (OperatorSessionBindingCommand::SetReasoningEffort {
                reasoning_effort: Some("xhigh".to_string())
            })
    ));

    let mut tier_ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };
    assert!(press(&mut tier_ui, &snapshot, KeyCode::Char('f')).await);
    assert!(press(&mut tier_ui, &snapshot, KeyCode::Down).await);
    assert!(press(&mut tier_ui, &snapshot, KeyCode::Down).await);
    assert!(press(&mut tier_ui, &snapshot, KeyCode::Enter).await);
    assert!(matches!(
        tier_ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.command == (OperatorSessionBindingCommand::SetServiceTier {
                service_tier: Some("fast".to_string())
            })
    ));
}

#[tokio::test]
async fn legacy_session_effort_shortcuts_remain_compatible_aliases() {
    let snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    let mut dashboard = UiState {
        page: Page::Dashboard,
        focus: crate::tui::types::Focus::Sessions,
        ..UiState::default()
    };

    assert!(press(&mut dashboard, &snapshot, KeyCode::Enter).await);
    assert_eq!(dashboard.overlay, Overlay::SessionEffortMenu);

    let mut sessions = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };
    assert!(press(&mut sessions, &snapshot, KeyCode::Enter).await);
    assert_eq!(sessions.overlay, Overlay::SessionEffortMenu);

    sessions.overlay = Overlay::None;
    assert!(press(&mut sessions, &snapshot, KeyCode::Char('A')).await);
    assert_eq!(sessions.overlay, Overlay::SessionAffinityActions);

    sessions.overlay = Overlay::None;
    assert!(!sessions.sessions_page_active_only);
    assert!(press(&mut sessions, &snapshot, KeyCode::Char('a')).await);
    assert!(sessions.sessions_page_active_only);
    assert_eq!(sessions.overlay, Overlay::None);

    sessions.overlay = Overlay::None;
    assert!(press(&mut sessions, &snapshot, KeyCode::Char('x')).await);
    assert!(matches!(
        sessions.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.command == (OperatorSessionBindingCommand::SetReasoningEffort {
                reasoning_effort: None
            })
    ));
}

#[tokio::test]
async fn session_reset_and_remote_read_only_boundaries_are_explicit() {
    let mut snapshot = affinity_snapshot(0, Some("affinity:v1:current"));
    snapshot.rows[0].binding = crate::state::SessionBindingProjection {
        revision: "binding:v1:manual".to_string(),
        model: Some("gpt-5.4".to_string()),
        continuity_mode: Some(crate::state::SessionContinuityMode::ManualProfile),
        ..crate::state::SessionBindingProjection::default()
    };
    let mut ui = UiState {
        page: Page::Sessions,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Char('R')).await);
    assert!(matches!(
        ui.pending_operator_action,
        Some(PendingOperatorAction::MutateSessionBinding(ref request))
            if request.expected_binding_revision == "binding:v1:manual"
                && request.command == OperatorSessionBindingCommand::ResetManualOverrides
    ));

    let mut remote = UiState {
        page: Page::Sessions,
        runtime_connection: RuntimeConnectionKind::RemoteObserver,
        ..UiState::default()
    };
    assert!(press(&mut remote, &snapshot, KeyCode::Char('b')).await);
    assert_eq!(remote.overlay, Overlay::None);
    assert!(remote.pending_operator_action.is_none());
    assert!(
        remote
            .toast
            .as_ref()
            .is_some_and(|(message, _)| message.contains("read-only"))
    );
}

#[tokio::test]
async fn settings_scroll_keys_move_and_return_to_the_top() {
    let snapshot = Snapshot::default();
    let mut ui = UiState {
        page: Page::Settings,
        ..UiState::default()
    };

    assert!(press(&mut ui, &snapshot, KeyCode::Down).await);
    assert_eq!(ui.settings_scroll, 1);
    assert!(press(&mut ui, &snapshot, KeyCode::PageDown).await);
    assert_eq!(ui.settings_scroll, 9);
    assert!(press(&mut ui, &snapshot, KeyCode::PageUp).await);
    assert_eq!(ui.settings_scroll, 1);
    assert!(press(&mut ui, &snapshot, KeyCode::Home).await);
    assert_eq!(ui.settings_scroll, 0);
    assert!(press(&mut ui, &snapshot, KeyCode::End).await);
    assert_eq!(ui.settings_scroll, u16::MAX);
}

#[tokio::test]
async fn integrated_settings_controls_are_reachable_again() {
    let snapshot = Snapshot::default();

    let mut reload = UiState {
        page: Page::Settings,
        ..UiState::default()
    };
    assert!(press(&mut reload, &snapshot, KeyCode::Char('R')).await);
    assert!(matches!(
        reload.pending_operator_action,
        Some(PendingOperatorAction::ReloadRuntime)
    ));

    let mut configured_profile = UiState {
        page: Page::Settings,
        ..UiState::default()
    };
    assert!(press(&mut configured_profile, &snapshot, KeyCode::Char('p')).await);
    assert_eq!(
        configured_profile.overlay,
        Overlay::ConfiguredDefaultProfileMenu
    );

    let mut runtime_profile = UiState {
        page: Page::Settings,
        ..UiState::default()
    };
    assert!(press(&mut runtime_profile, &snapshot, KeyCode::Char('P')).await);
    assert_eq!(runtime_profile.overlay, Overlay::RuntimeDefaultProfileMenu);

    let mut capabilities = UiState {
        page: Page::Settings,
        ..UiState::default()
    };
    assert!(press(&mut capabilities, &snapshot, KeyCode::Char('C')).await);
    assert!(matches!(
        capabilities.pending_operator_action,
        Some(PendingOperatorAction::InspectRelayCapabilities(_))
    ));
}

#[tokio::test]
async fn settings_live_smoke_requires_confirmation_and_uses_exact_cases() {
    let mut snapshot = affinity_snapshot(0, None);
    snapshot.rows[0].last_model = Some("gpt-smoke".to_string());

    let mut compact = UiState {
        page: Page::Settings,
        ..UiState::default()
    };
    assert!(press(&mut compact, &snapshot, KeyCode::Char('X')).await);
    assert!(compact.pending_operator_action.is_none());
    assert!(press(&mut compact, &snapshot, KeyCode::Char('X')).await);
    let Some(PendingOperatorAction::RunRelayLiveSmoke(compact_start)) =
        compact.pending_operator_action.as_ref()
    else {
        panic!("confirmed X must queue compact live smoke");
    };
    assert_eq!(compact_start.request.model.as_deref(), Some("gpt-smoke"));
    assert_eq!(
        compact_start.request.acknowledgement.as_deref(),
        Some(CODEX_RELAY_LIVE_SMOKE_ACK)
    );
    assert_eq!(
        compact_start.request.cases,
        vec![CodexRelayLiveSmokeCase::ResponsesCompact]
    );

    let mut compact_and_image = UiState {
        page: Page::Settings,
        ..UiState::default()
    };
    assert!(press(&mut compact_and_image, &snapshot, KeyCode::Char('Y')).await);
    assert!(compact_and_image.pending_operator_action.is_none());
    assert!(press(&mut compact_and_image, &snapshot, KeyCode::Char('Y')).await);
    let Some(PendingOperatorAction::RunRelayLiveSmoke(image_start)) =
        compact_and_image.pending_operator_action.as_ref()
    else {
        panic!("confirmed Y must queue compact and image live smoke");
    };
    assert_eq!(
        image_start.request.cases,
        vec![
            CodexRelayLiveSmokeCase::ResponsesCompact,
            CodexRelayLiveSmokeCase::HostedImageGeneration,
        ]
    );
}

#[tokio::test]
async fn remote_observer_cannot_trigger_settings_runtime_or_relay_actions() {
    let mut snapshot = affinity_snapshot(0, None);
    snapshot.rows[0].last_model = Some("gpt-smoke".to_string());

    for code in ['R', 'p', 'P', 'C', 'X', 'Y'] {
        let mut ui = UiState {
            page: Page::Settings,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..UiState::default()
        };

        assert!(press(&mut ui, &snapshot, KeyCode::Char(code)).await);
        assert!(ui.pending_operator_action.is_none(), "key={code}");
        assert_eq!(ui.overlay, Overlay::None, "key={code}");
        assert!(!ui.codex_relay_diagnostics.loading, "key={code}");
        assert!(!ui.codex_relay_live_smoke.loading, "key={code}");
        assert!(
            ui.codex_relay_live_smoke.pending_confirm.is_none(),
            "key={code}"
        );
        assert!(
            ui.toast
                .as_ref()
                .is_some_and(|(message, _)| message.contains("read-only")),
            "key={code}, toast={:?}",
            ui.toast
        );
    }
}

#[tokio::test]
async fn local_attached_settings_relay_actions_require_advertised_capabilities() {
    let snapshot = Snapshot::default();
    let mut unavailable = UiState {
        page: Page::Settings,
        runtime_connection: RuntimeConnectionKind::LocalAttached,
        local_operator_transport_available: true,
        ..UiState::default()
    };
    assert!(press(&mut unavailable, &snapshot, KeyCode::Char('C')).await);
    assert!(unavailable.pending_operator_action.is_none());

    let mut available = UiState {
        page: Page::Settings,
        runtime_connection: RuntimeConnectionKind::LocalAttached,
        local_operator_transport_available: true,
        operator_action_capabilities: OperatorActionCapabilities {
            inspect_relay_capabilities: true,
            run_relay_live_smoke: true,
            ..OperatorActionCapabilities::default()
        },
        ..UiState::default()
    };
    assert!(press(&mut available, &snapshot, KeyCode::Char('C')).await);
    assert!(matches!(
        available.pending_operator_action,
        Some(PendingOperatorAction::InspectRelayCapabilities(_))
    ));
}

#[tokio::test]
async fn removed_routing_and_session_mutation_keys_are_inert() {
    let snapshot = Snapshot::default();
    let cases = [
        (Page::Routing, vec!['r', 'h', 'H', 'c', 'C', 'o', 'O']),
        (Page::Sessions, vec!['P']),
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
