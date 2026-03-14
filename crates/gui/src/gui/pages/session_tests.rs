use std::collections::HashSet;

use super::*;

fn sample_session_row() -> SessionRow {
    SessionRow {
        session_id: Some("sid-1".to_string()),
        observation_scope: SessionObservationScope::ObservedOnly,
        host_local_transcript_path: None,
        last_client_name: None,
        last_client_addr: None,
        cwd: Some("G:/codes/rust/codex-helper".to_string()),
        active_count: 0,
        active_started_at_ms_min: None,
        last_status: None,
        last_duration_ms: None,
        last_ended_at_ms: None,
        last_model: None,
        last_reasoning_effort: None,
        last_service_tier: None,
        last_provider_id: None,
        last_station: None,
        last_upstream_base_url: None,
        last_usage: None,
        total_usage: None,
        turns_total: None,
        turns_with_usage: None,
        binding_profile_name: None,
        binding_continuity_mode: None,
        last_route_decision: None,
        effective_model: None,
        effective_reasoning_effort: None,
        effective_service_tier: None,
        effective_station_value: None,
        effective_upstream_base_url: None,
        override_model: None,
        override_effort: None,
        override_station: None,
        override_service_tier: None,
    }
}

#[test]
fn explain_effective_route_uses_profile_context() {
    let mut row = sample_session_row();
    row.binding_profile_name = Some("fast".to_string());
    row.effective_service_tier = Some(ResolvedRouteValue {
        value: "priority".to_string(),
        source: RouteValueSource::ProfileDefault,
    });

    let explanation =
        explain_effective_route_field(&row, EffectiveRouteField::ServiceTier, Language::Zh);

    assert_eq!(explanation.value, "priority");
    assert_eq!(explanation.source_label, "profile 默认");
    assert!(explanation.reason.contains("profile fast"));
    assert!(explanation.reason.contains("service_tier"));
}

#[test]
fn explain_effective_route_handles_station_mapping_for_model() {
    let mut row = sample_session_row();
    row.last_model = Some("gpt-5.4".to_string());
    row.last_station = Some("right".to_string());
    row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
    row.effective_station_value = Some(ResolvedRouteValue {
        value: "right".to_string(),
        source: RouteValueSource::RuntimeFallback,
    });
    row.effective_model = Some(ResolvedRouteValue {
        value: "gpt-5.4-fast".to_string(),
        source: RouteValueSource::StationMapping,
    });

    let explanation = explain_effective_route_field(&row, EffectiveRouteField::Model, Language::Zh);

    assert_eq!(explanation.source_label, "站点映射");
    assert!(explanation.reason.contains("gpt-5.4"));
    assert!(explanation.reason.contains("right"));
    assert!(explanation.reason.contains("gpt-5.4-fast"));
}

#[test]
fn explain_effective_route_marks_upstream_unresolved_after_station_switch() {
    let mut row = sample_session_row();
    row.last_station = Some("right".to_string());
    row.last_upstream_base_url = Some("https://www.right.codes/codex/v1".to_string());
    row.effective_station_value = Some(ResolvedRouteValue {
        value: "vibe".to_string(),
        source: RouteValueSource::GlobalOverride,
    });

    let explanation =
        explain_effective_route_field(&row, EffectiveRouteField::Upstream, Language::Zh);

    assert_eq!(explanation.value, "-");
    assert_eq!(explanation.source_label, "未解析");
    assert!(explanation.reason.contains("vibe"));
    assert!(explanation.reason.contains("right"));
}

#[test]
fn session_control_posture_warns_when_bound_profile_is_missing() {
    let mut row = sample_session_row();
    row.binding_profile_name = Some("fast".to_string());
    row.binding_continuity_mode = Some(SessionContinuityMode::ManualProfile);

    let posture = session_control_posture(&row, &[], Language::Zh);

    assert_eq!(posture.tone, SessionControlTone::Warning);
    assert!(posture.headline.contains("已缺失"));
    assert!(posture.detail.contains("找不到这个 profile"));
}

#[test]
fn session_control_posture_describes_session_overrides_without_binding() {
    let mut row = sample_session_row();
    row.override_station = Some("right".to_string());
    row.override_service_tier = Some("priority".to_string());

    let posture = session_control_posture(&row, &[], Language::En);

    assert_eq!(posture.tone, SessionControlTone::Neutral);
    assert!(posture.headline.contains("no profile binding"));
    assert!(posture.detail.contains("station"));
    assert!(posture.detail.contains("service_tier"));
}

#[test]
fn route_decision_changed_fields_reports_effective_drift() {
    let mut row = sample_session_row();
    row.effective_model = Some(ResolvedRouteValue {
        value: "gpt-5.4-fast".to_string(),
        source: RouteValueSource::SessionOverride,
    });
    row.effective_station_value = Some(ResolvedRouteValue {
        value: "right".to_string(),
        source: RouteValueSource::RuntimeFallback,
    });
    row.last_route_decision = Some(RouteDecisionProvenance {
        decided_at_ms: 123,
        effective_model: Some(ResolvedRouteValue {
            value: "gpt-5.4".to_string(),
            source: RouteValueSource::ProfileDefault,
        }),
        effective_station: Some(ResolvedRouteValue {
            value: "right".to_string(),
            source: RouteValueSource::RuntimeFallback,
        }),
        ..Default::default()
    });

    let changed = route_decision_changed_fields(&row, Language::En);

    assert_eq!(changed, vec!["model".to_string()]);
}

#[test]
fn session_route_decision_status_line_mentions_changed_fields() {
    let mut row = sample_session_row();
    row.effective_service_tier = Some(ResolvedRouteValue {
        value: "priority".to_string(),
        source: RouteValueSource::SessionOverride,
    });
    row.last_route_decision = Some(RouteDecisionProvenance {
        decided_at_ms: 456,
        effective_service_tier: Some(ResolvedRouteValue {
            value: "default".to_string(),
            source: RouteValueSource::ProfileDefault,
        }),
        ..Default::default()
    });

    let status = session_route_decision_status_line(&row, Language::En);

    assert!(status.contains("snapshot"));
    assert!(status.contains("service_tier"));
}

#[test]
fn build_session_rows_from_cards_preserves_last_route_decision() {
    let rows = build_session_rows_from_cards(&[SessionIdentityCard {
        session_id: Some("sid-1".to_string()),
        last_route_decision: Some(RouteDecisionProvenance {
            decided_at_ms: 789,
            provider_id: Some("right".to_string()),
            effective_model: Some(ResolvedRouteValue {
                value: "gpt-5.4-fast".to_string(),
                source: RouteValueSource::StationMapping,
            }),
            ..Default::default()
        }),
        ..Default::default()
    }]);

    assert_eq!(rows.len(), 1);
    let decision = rows[0]
        .last_route_decision
        .as_ref()
        .expect("route decision");
    assert_eq!(decision.decided_at_ms, 789);
    assert_eq!(decision.provider_id.as_deref(), Some("right"));
    assert_eq!(
        decision
            .effective_model
            .as_ref()
            .map(|value| value.value.as_str()),
        Some("gpt-5.4-fast")
    );
}

#[test]
fn session_list_control_label_prefers_profile_binding() {
    let mut row = sample_session_row();
    row.binding_profile_name = Some("fast".to_string());
    row.override_station = Some("right".to_string());

    assert_eq!(session_list_control_label(&row), "pf:fast");
}

#[test]
fn focus_session_in_sessions_resets_filters_and_focuses_sid() {
    let mut state = SessionsViewState {
        active_only: true,
        errors_only: true,
        overrides_only: true,
        lock_order: true,
        search: "old".to_string(),
        default_profile_selection: None,
        selected_session_id: None,
        selected_idx: 9,
        ordered_session_ids: Vec::new(),
        last_active_set: HashSet::new(),
        editor: SessionOverrideEditor::default(),
    };

    focus_session_in_sessions(&mut state, "sid-history".to_string());

    assert!(!state.active_only);
    assert!(!state.errors_only);
    assert!(!state.overrides_only);
    assert_eq!(state.search, "sid-history");
    assert_eq!(state.selected_session_id.as_deref(), Some("sid-history"));
    assert_eq!(state.selected_idx, 0);
    assert!(state.lock_order);
}
