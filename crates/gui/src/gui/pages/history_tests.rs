use std::collections::HashMap;
use std::path::PathBuf;

use super::super::i18n::Language;
use super::history::{HistoryScope, HistoryViewState};
use super::history_external::{
    ExternalHistoryFocus, ExternalHistoryOrigin, ensure_external_focus_visible,
    merge_external_focus_session, prepare_select_session_from_external,
};
use super::history_observed::*;
use super::*;
use crate::dashboard_core::{
    ControlProfileOption, HostLocalControlPlaneCapabilities, ProviderOption,
    RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities, StationOption, WindowStats,
};
use crate::gui::proxy_control::GuiRuntimeSnapshot;
use crate::sessions::SessionSummarySource;
use crate::state::{
    FinishedRequest, ResolvedRouteValue, RouteValueSource, SessionIdentityCard, UsageRollupView,
};

fn sample_summary(id: &str, source: SessionSummarySource) -> SessionSummary {
    SessionSummary {
        id: id.to_string(),
        path: match source {
            SessionSummarySource::LocalFile => PathBuf::from(format!("/tmp/{id}.jsonl")),
            SessionSummarySource::ObservedOnly => PathBuf::new(),
        },
        cwd: Some("/workdir".to_string()),
        created_at: None,
        updated_at: Some("1m".to_string()),
        last_response_at: Some("1m".to_string()),
        user_turns: 1,
        assistant_turns: 1,
        rounds: 1,
        first_user_message: Some("summary".to_string()),
        source,
        sort_hint_ms: Some(1_000),
    }
}

fn empty_snapshot() -> GuiRuntimeSnapshot {
    GuiRuntimeSnapshot {
        kind: crate::gui::proxy_control::ProxyModeKind::Attached,
        base_url: Some("http://127.0.0.1:3210".to_string()),
        port: Some(3210),
        service_name: Some("codex".to_string()),
        last_error: None,
        active: Vec::new(),
        recent: Vec::new(),
        session_cards: Vec::new(),
        global_station_override: None,
        configured_active_station: None,
        effective_active_station: None,
        configured_default_profile: None,
        default_profile: None,
        profiles: Vec::<ControlProfileOption>::new(),
        providers: Vec::<ProviderOption>::new(),
        session_model_overrides: HashMap::new(),
        session_station_overrides: HashMap::new(),
        session_effort_overrides: HashMap::new(),
        session_service_tier_overrides: HashMap::new(),
        session_stats: HashMap::new(),
        stations: Vec::<StationOption>::new(),
        usage_rollup: UsageRollupView::default(),
        stats_5m: WindowStats::default(),
        stats_1h: WindowStats::default(),
        operator_runtime_summary: None,
        operator_retry_summary: None,
        operator_health_summary: None,
        operator_counts: None,
        supports_operator_summary_api: false,
        configured_retry: None,
        resolved_retry: None,
        supports_v1: true,
        supports_retry_config_api: true,
        supports_persisted_station_settings: true,
        supports_default_profile_override: true,
        supports_station_runtime_override: true,
        supports_session_override_reset: true,
        shared_capabilities: SharedControlPlaneCapabilities {
            session_observability: true,
            request_history: true,
        },
        host_local_capabilities: HostLocalControlPlaneCapabilities {
            session_history: true,
            transcript_read: true,
            cwd_enrichment: true,
        },
        remote_admin_access: RemoteAdminAccessCapabilities::default(),
    }
}

#[test]
fn prepare_select_session_from_external_resets_scope_and_focus() {
    let mut state = HistoryViewState::default();
    state.scope = HistoryScope::CurrentProject;
    state.query = "old".to_string();
    state.applied_query = "old".to_string();

    prepare_select_session_from_external(
        &mut state,
        sample_summary("sid-ext", SessionSummarySource::ObservedOnly),
        ExternalHistoryOrigin::Sessions,
    );

    assert_eq!(state.scope, HistoryScope::GlobalRecent);
    assert!(state.query.is_empty());
    assert_eq!(state.selected_id.as_deref(), Some("sid-ext"));
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].id, "sid-ext");
    assert!(state.external_focus.is_some());
    assert!(state.loaded_at_ms.is_none());
}

#[test]
fn merge_external_focus_session_preserves_local_file_when_richer() {
    let mut list = vec![sample_summary("sid-1", SessionSummarySource::LocalFile)];
    let focus = ExternalHistoryFocus {
        summary: sample_summary("sid-1", SessionSummarySource::ObservedOnly),
        origin: ExternalHistoryOrigin::Sessions,
    };

    merge_external_focus_session(&mut list, &focus);

    assert_eq!(list.len(), 1);
    assert_eq!(list[0].source, SessionSummarySource::LocalFile);
    assert!(!list[0].path.as_os_str().is_empty());
}

#[test]
fn ensure_external_focus_visible_inserts_selected_external_summary() {
    let mut state = HistoryViewState::default();
    state.external_focus = Some(ExternalHistoryFocus {
        summary: sample_summary("sid-ext", SessionSummarySource::ObservedOnly),
        origin: ExternalHistoryOrigin::Sessions,
    });
    state.selected_id = Some("sid-ext".to_string());

    ensure_external_focus_visible(&mut state);

    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].id, "sid-ext");
    assert_eq!(state.sessions[0].source, SessionSummarySource::ObservedOnly);
}

#[test]
fn observed_history_summaries_from_cards_are_marked_observed_only() {
    let mut snapshot = empty_snapshot();
    snapshot.session_cards = vec![SessionIdentityCard {
        session_id: Some("sid-card".to_string()),
        last_client_name: Some("Frank-Desk".to_string()),
        last_client_addr: Some("100.64.0.12".to_string()),
        cwd: Some("/remote/workdir".to_string()),
        last_ended_at_ms: Some(2_000),
        last_status: Some(200),
        last_provider_id: Some("right".to_string()),
        binding_profile_name: Some("fast".to_string()),
        effective_model: Some(ResolvedRouteValue {
            value: "gpt-5.4-fast".to_string(),
            source: RouteValueSource::StationMapping,
        }),
        effective_station: Some(ResolvedRouteValue {
            value: "right".to_string(),
            source: RouteValueSource::RuntimeFallback,
        }),
        effective_service_tier: Some(ResolvedRouteValue {
            value: "priority".to_string(),
            source: RouteValueSource::ProfileDefault,
        }),
        ..SessionIdentityCard::default()
    }];

    let summaries = build_observed_history_summaries(&snapshot, Language::Zh);

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "sid-card");
    assert_eq!(summaries[0].source, SessionSummarySource::ObservedOnly);
    assert_eq!(summaries[0].sort_hint_ms, Some(2_000));
    assert!(
        summaries[0]
            .first_user_message
            .as_deref()
            .is_some_and(|msg| {
                msg.contains("station=right")
                    && msg.contains("model=gpt-5.4-fast")
                    && msg.contains("client=Frank-Desk @ 100.64.0.12")
            })
    );
    assert!(!history_session_supports_local_actions(&summaries[0]));
}

#[test]
fn observed_session_row_from_snapshot_prefers_session_cards() {
    let mut snapshot = empty_snapshot();
    snapshot.session_cards = vec![SessionIdentityCard {
        session_id: Some("sid-card".to_string()),
        last_service_tier: Some("priority".to_string()),
        effective_service_tier: Some(ResolvedRouteValue {
            value: "priority".to_string(),
            source: RouteValueSource::ProfileDefault,
        }),
        effective_station: Some(ResolvedRouteValue {
            value: "right".to_string(),
            source: RouteValueSource::RuntimeFallback,
        }),
        last_route_decision: Some(crate::state::RouteDecisionProvenance {
            decided_at_ms: 1_000,
            provider_id: Some("right".to_string()),
            ..Default::default()
        }),
        ..SessionIdentityCard::default()
    }];

    let row = observed_session_row_from_snapshot(&snapshot, "sid-card").expect("observed row");

    assert_eq!(row.session_id.as_deref(), Some("sid-card"));
    assert_eq!(
        row.effective_service_tier
            .as_ref()
            .map(|value| value.value.as_str()),
        Some("priority")
    );
    assert_eq!(
        row.last_route_decision
            .as_ref()
            .and_then(|decision| decision.provider_id.as_deref()),
        Some("right")
    );
}

#[test]
fn history_service_tier_display_marks_priority_as_fast_mode() {
    assert_eq!(
        history_service_tier_display(Some("priority"), Language::En),
        "priority (fast mode)"
    );
}

#[test]
fn observed_history_summaries_fall_back_to_recent_requests() {
    let mut snapshot = empty_snapshot();
    snapshot.recent = vec![FinishedRequest {
        id: 1,
        session_id: Some("sid-recent".to_string()),
        client_name: Some("Tablet".to_string()),
        client_addr: Some("100.64.0.13".to_string()),
        cwd: Some("/remote/recent".to_string()),
        model: Some("gpt-5.4".to_string()),
        reasoning_effort: None,
        service_tier: Some("priority".to_string()),
        station_name: Some("vibe".to_string()),
        provider_id: Some("vibe".to_string()),
        upstream_base_url: None,
        route_decision: None,
        usage: None,
        retry: None,
        service: "codex".to_string(),
        method: "POST".to_string(),
        path: "/v1/responses".to_string(),
        status_code: 200,
        duration_ms: 500,
        ttfb_ms: None,
        ended_at_ms: 9_000,
    }];

    let summaries = build_observed_history_summaries(&snapshot, Language::En);

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "sid-recent");
    assert_eq!(summaries[0].source, SessionSummarySource::ObservedOnly);
    assert_eq!(summaries[0].sort_hint_ms, Some(9_000));
    assert!(
        summaries[0]
            .first_user_message
            .as_deref()
            .is_some_and(|msg| {
                msg.contains("station=vibe")
                    && msg.contains("provider=vibe")
                    && msg.contains("client=Tablet @ 100.64.0.13")
            })
    );
}
