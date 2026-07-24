use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use super::model::{Palette, ProviderOption, Snapshot};
use super::state::UiState;
use super::types::Overlay;

mod chrome;
mod modals;
mod pages;
mod stats;
mod widgets;

pub(in crate::tui) fn render_app(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    service_name: &'static str,
    port: u16,
    providers: &[ProviderOption],
) {
    f.render_widget(widgets::BackgroundWidget { p }, f.area());

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(f.area());

    chrome::render_header(f, p, ui, snapshot, service_name, port, outer[0]);
    pages::render_body(f, p, ui, snapshot, providers, outer[1]);
    chrome::render_footer(f, p, ui, outer[2]);

    match ui.overlay {
        Overlay::None => {}
        Overlay::Help => modals::render_help_modal(f, p, ui),
        Overlay::ProviderInfo => modals::render_provider_info_modal(f, p, ui, snapshot, providers),
        Overlay::StartupAlert => modals::render_startup_alert_modal(f, p, ui),
        Overlay::RoutingActions => modals::render_routing_actions_modal(f, p, ui, snapshot),
        Overlay::RoutingConfirmation => modals::render_routing_confirmation_modal(f, p, ui),
        Overlay::SessionAffinityActions => {
            modals::render_session_affinity_actions_modal(f, p, ui, snapshot)
        }
        Overlay::SessionAffinityConfirmation => {
            modals::render_session_affinity_confirmation_modal(f, p, ui, snapshot)
        }
        Overlay::SessionProfileMenu => modals::render_session_profile_menu(f, p, ui),
        Overlay::SessionModelMenu => modals::render_session_model_menu(f, p, ui),
        Overlay::SessionEffortMenu => modals::render_session_effort_menu(f, p, ui),
        Overlay::SessionServiceTierMenu => modals::render_session_service_tier_menu(f, p, ui),
        Overlay::SessionBindingInput => modals::render_session_binding_input(f, p, ui),
        Overlay::ConfiguredDefaultProfileMenu | Overlay::RuntimeDefaultProfileMenu => {
            modals::render_default_profile_menu(f, p, ui)
        }
        Overlay::SessionTranscript => modals::render_session_transcript_modal(f, p, ui),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::codex_integration::{
        CodexStartupReadiness, CodexStartupReadinessIssue, CodexStartupReadinessIssueKind,
        CodexStartupReadinessSeverity,
    };
    use crate::dashboard_core::{
        ControlProfileOption, OperatorPolicyActionSummary, OperatorProviderCapacity,
        OperatorRequestObservability, OperatorRequestSummary, OperatorRouteCandidateSummary,
        OperatorRoutingSummary,
    };
    use crate::sessions::{SessionSummary, SessionSummarySource};
    use crate::state::{
        BalanceSnapshotStatus, ProviderBalanceSnapshot, SessionObservationScope, UsageBucket,
    };
    use crate::tui::Language;
    use crate::tui::model::{SessionRouteAffinityView, SessionRow, Snapshot, UpstreamSummary};
    use crate::tui::state::{RecentCodexRow, RuntimeConnectionKind};
    use crate::tui::types::{Focus, Overlay, Page, StatsFocus};
    use codex_helper_core::fleet::{
        FleetConfidence, FleetEvidence, FleetEvidenceSource, FleetNodeHealth, FleetNodeKind,
        FleetNodeSnapshot, FleetProcessSummary, FleetSnapshot, FleetTopology, FleetUsageSummary,
        FleetWorkUnit, FleetWorkUnitKind, FleetWorkUnitState,
    };

    fn buffer_text(buffer: &Buffer) -> String {
        let mut out = String::new();
        for y in buffer.area.y..buffer.area.y.saturating_add(buffer.area.height) {
            for x in buffer.area.x..buffer.area.x.saturating_add(buffer.area.width) {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn text_without_whitespace(text: &str) -> String {
        text.chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
    }

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            request_control_evidence: HashMap::new(),
            usage_day: crate::state::UsageDayView::default(),
            quota_analytics: crate::quota_analytics::QuotaAnalyticsView::default(),
            usage_rollup: crate::state::UsageRollupView {
                by_provider_endpoint: vec![(
                    "超级路由入口".to_string(),
                    UsageBucket {
                        requests_total: 7,
                        ..UsageBucket::default()
                    },
                )],
                by_provider: vec![
                    (
                        "input-light".to_string(),
                        UsageBucket {
                            requests_total: 5,
                            ..UsageBucket::default()
                        },
                    ),
                    (
                        "超级中转套餐年度输入提供商".to_string(),
                        UsageBucket {
                            requests_total: 2,
                            requests_error: 1,
                            ..UsageBucket::default()
                        },
                    ),
                ],
                ..crate::state::UsageRollupView::default()
            },
            provider_balances: HashMap::from([
                (
                    "input-light".to_string(),
                    vec![ProviderBalanceSnapshot {
                        observation_provider_id: "input-light-observer".to_string(),
                        provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                            "codex",
                            "input-light",
                            "default",
                        ),
                        status: BalanceSnapshotStatus::Exhausted,
                        exhausted: Some(true),
                        exhaustion_affects_routing: false,
                        quota_period: Some("daily".to_string()),
                        quota_remaining_usd: Some("0".to_string()),
                        quota_limit_usd: Some("300".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    }],
                ),
                (
                    "超级中转套餐年度输入提供商".to_string(),
                    vec![ProviderBalanceSnapshot {
                        observation_provider_id: "年度套餐观测".to_string(),
                        provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                            "codex",
                            "超级中转套餐年度输入提供商",
                            "default",
                        ),
                        status: BalanceSnapshotStatus::Stale,
                        error: Some("refresh balance failed".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    }],
                ),
            ]),
            routing: Some(OperatorRoutingSummary {
                route_graph_key: "routing:sha256:sample".to_string(),
                control_revision: 0,
                provider_policy_revision: 0,
                entry: "main".to_string(),
                entry_strategy: crate::config::RouteStrategy::RoundRobin,
                entry_target: Some("input-light.default".to_string()),
                new_session_preference: None,
                affinity_policy: crate::config::RouteAffinityPolicy::FallbackSticky,
                scheduling_preset: crate::config::SchedulingPreset::Balanced,
                fallback_ttl_ms: Some(300_000),
                reprobe_preferred_after_ms: Some(30_000),
                candidates: [
                    "input",
                    "input1",
                    "input2",
                    "input-light",
                    "超级中转套餐年度输入提供商",
                ]
                .into_iter()
                .enumerate()
                .map(|(route_order, provider_id)| OperatorRouteCandidateSummary {
                    route_order,
                    provider_id: provider_id.to_string(),
                    endpoint_id: "default".to_string(),
                    preference_group: if route_order < 2 { 0 } else { 1 },
                    route_path: vec!["main".to_string()],
                })
                .collect(),
            }),
            pricing_catalog: Default::default(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        }
    }

    fn sample_providers() -> Vec<ProviderOption> {
        [
            "input",
            "input1",
            "input2",
            "input-light",
            "超级中转套餐年度输入提供商",
        ]
        .into_iter()
        .enumerate()
        .map(|(idx, name)| ProviderOption {
            name: name.to_string(),
            alias: None,
            configured_enabled: true,
            effective_enabled: true,
            routable_endpoints: 1,
            credential_readiness: Some(crate::credentials::CredentialAggregateReadiness::Ready),
            endpoints: vec![UpstreamSummary {
                provider_name: name.to_string(),
                name: "default".to_string(),
                provider_endpoint_key: format!("endpoint:sha256:{idx}"),
                origin: Some(format!("https://provider-{idx}.example.test")),
                priority: idx as u32,
                configured_enabled: true,
                effective_enabled: true,
                routable: true,
                credential_readiness: Some(crate::credentials::CredentialReadinessCode::Ready),
                credential_details: Vec::new(),
                runtime_enabled_override: None,
                runtime_state: Default::default(),
                runtime_state_override: None,
                capacity: OperatorProviderCapacity {
                    configured_max_concurrent_requests: Some(if idx == 3 { 20 } else { 15 }),
                    effective_max_concurrent_requests: Some(if idx == 3 { 20 } else { 15 }),
                    active: Some(idx as u32),
                    limit: Some(if idx == 3 { 20 } else { 15 }),
                    saturated: false,
                    inherited_from_provider: Some(false),
                },
                policy_actions: if idx == 3 {
                    vec![OperatorPolicyActionSummary {
                        active_cooldown: true,
                        code: "provider_cooldown".to_string(),
                        cooldown_remaining_secs: Some(42),
                    }]
                } else {
                    Vec::new()
                },
            }],
            capacity: OperatorProviderCapacity {
                configured_max_concurrent_requests: Some(if idx == 3 { 20 } else { 15 }),
                effective_max_concurrent_requests: Some(if idx == 3 { 20 } else { 15 }),
                active: Some(idx as u32),
                limit: Some(if idx == 3 { 20 } else { 15 }),
                saturated: false,
                inherited_from_provider: None,
            },
        })
        .collect()
    }

    fn dashboard_request(id: u64, session_id: &str) -> OperatorRequestSummary {
        OperatorRequestSummary {
            id,
            trace_key: None,
            session_key: Some(session_id.to_string()),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            route_path: Vec::new(),
            upstream_origin: None,
            usage: None,
            cache_accounting_convention: Default::default(),
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
            observability: OperatorRequestObservability {
                duration_ms: Some(10),
                ttfb_ms: None,
                generation_ms: None,
                output_tokens_per_second: None,
                attempt_count: 1,
                route_attempt_count: 0,
                retried: false,
                cross_provider_failover: false,
                same_provider_retry: false,
                fast_mode: false,
                streaming: false,
            },
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 10,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: id,
        }
    }

    fn sample_startup_readiness() -> CodexStartupReadiness {
        CodexStartupReadiness {
            issues: vec![CodexStartupReadinessIssue {
                kind: CodexStartupReadinessIssueKind::DiagnosticError,
                severity: CodexStartupReadinessSeverity::Warning,
                title: "Codex switch requires recovery".to_string(),
                detail: "The config no longer matches the switch journal.".to_string(),
                action: "Do not overwrite Codex config; reconcile the recorded fingerprints first."
                    .to_string(),
            }],
        }
    }

    fn sample_fleet_snapshot() -> FleetSnapshot {
        FleetSnapshot {
            api_version: 1,
            service_name: "codex".to_string(),
            refreshed_at_ms: 1_000,
            nodes: vec![FleetNodeSnapshot {
                node_id: "local".to_string(),
                label: "local workstation".to_string(),
                kind: FleetNodeKind::Local,
                health: FleetNodeHealth::Fresh,
                credential_readiness: Some(crate::credentials::CredentialAggregateReadiness::Ready),
                refreshed_at_ms: 1_000,
                stale_since_ms: None,
                snapshot_age_ms: Some(0),
                active_endpoint: Some("http://127.0.0.1:4211".to_string()),
                last_error: None,
                processes: FleetProcessSummary {
                    scan_available: true,
                    codex_like_processes: 2,
                    error: None,
                },
                topology: FleetTopology::default(),
                work_units: vec![
                    FleetWorkUnit {
                        node_id: "local".to_string(),
                        id: "session:sid-1234567890".to_string(),
                        parent_id: None,
                        kind: FleetWorkUnitKind::Root,
                        state: FleetWorkUnitState::Running,
                        evidence: FleetEvidence {
                            source: FleetEvidenceSource::RuntimeStatus,
                            confidence: FleetConfidence::High,
                            detail: Some("runtime active request".to_string()),
                        },
                        session_id: Some("sid-1234567890".to_string()),
                        local_thread_id: Some("sid-1234567890".to_string()),
                        task_name: Some("implement fleet tui".to_string()),
                        cwd: Some("F:/SourceCodes/Rust/codex-helper".to_string()),
                        model: Some("gpt-5".to_string()),
                        provider_id: Some("input".to_string()),
                        last_status: Some(200),
                        active_started_at_ms: Some(900),
                        last_activity_ms: Some(950),
                        last_error: None,
                        usage: FleetUsageSummary::default(),
                    },
                    FleetWorkUnit {
                        node_id: "local".to_string(),
                        id: "subagent:research".to_string(),
                        parent_id: Some("session:sid-1234567890".to_string()),
                        kind: FleetWorkUnitKind::Subagent,
                        state: FleetWorkUnitState::WaitingApproval,
                        evidence: FleetEvidence {
                            source: FleetEvidenceSource::SessionLog,
                            confidence: FleetConfidence::Medium,
                            detail: Some("session log child task".to_string()),
                        },
                        session_id: Some("sid-1234567890".to_string()),
                        local_thread_id: Some("subagent-thread".to_string()),
                        task_name: Some("research fleet behavior".to_string()),
                        cwd: Some("F:/SourceCodes/Rust/codex-helper".to_string()),
                        model: Some("gpt-5".to_string()),
                        provider_id: Some("input".to_string()),
                        last_status: Some(202),
                        active_started_at_ms: Some(960),
                        last_activity_ms: Some(980),
                        last_error: None,
                        usage: FleetUsageSummary::default(),
                    },
                    FleetWorkUnit {
                        node_id: "local".to_string(),
                        id: "process:scan".to_string(),
                        parent_id: Some("subagent:research".to_string()),
                        kind: FleetWorkUnitKind::Process,
                        state: FleetWorkUnitState::Idle,
                        evidence: FleetEvidence {
                            source: FleetEvidenceSource::ProcessScan,
                            confidence: FleetConfidence::Low,
                            detail: Some("process scan child".to_string()),
                        },
                        session_id: Some("sid-1234567890".to_string()),
                        local_thread_id: Some("subagent-thread".to_string()),
                        task_name: Some("child process".to_string()),
                        cwd: Some("F:/SourceCodes/Rust/codex-helper".to_string()),
                        model: Some("gpt-5".to_string()),
                        provider_id: Some("input".to_string()),
                        last_status: None,
                        active_started_at_ms: None,
                        last_activity_ms: Some(990),
                        last_error: None,
                        usage: FleetUsageSummary::default(),
                    },
                ],
            }],
        }
    }

    fn unknown_session_row() -> SessionRow {
        SessionRow {
            session_id: None,
            local_session_id: None,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: Some("codex".to_string()),
            last_client_addr: None,
            cwd: None,
            active_count: 1,
            active_started_at_ms_min: Some(1),
            active_last_method: None,
            active_last_path: None,
            last_status: Some(200),
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            last_output_tokens_per_second: None,
            avg_output_tokens_per_second: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            binding: crate::state::SessionBindingProjection::default(),
            last_route_decision: None,
            route_affinity: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
        }
    }

    fn render_app_text(width: u16, height: u16, ui: &mut UiState, snapshot: &Snapshot) -> String {
        render_app_text_with_providers(width, height, ui, snapshot, &[])
    }

    fn render_app_text_with_providers(
        width: u16,
        height: u16,
        ui: &mut UiState,
        snapshot: &Snapshot,
        providers: &[ProviderOption],
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let frame = terminal
            .draw(|frame| {
                render_app(
                    frame,
                    Palette::default(),
                    ui,
                    snapshot,
                    "codex",
                    18080,
                    providers,
                );
            })
            .expect("draw");
        buffer_text(frame.buffer)
    }

    fn menu_profile(index: usize) -> ControlProfileOption {
        ControlProfileOption {
            name: format!("profile-{index:02}"),
            extends: None,
            model: Some(format!("gpt-{index:02}")),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            fast_mode: true,
            is_default: index == 20,
        }
    }

    async fn press_menu_end(ui: &mut UiState, snapshot: &Snapshot) {
        let mut providers = Vec::new();
        assert!(
            crate::tui::input::handle_key_event(
                crate::tui::input::KeyEventContext {
                    providers: &mut providers,
                    ui,
                    snapshot,
                },
                KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            )
            .await
        );
    }

    #[tokio::test]
    async fn long_profile_default_and_model_menus_keep_end_selection_visible_at_80x24() {
        let snapshot = sample_snapshot();
        let profiles = (1..=20).map(menu_profile).collect::<Vec<_>>();

        let mut session_profile = UiState {
            overlay: Overlay::SessionProfileMenu,
            profile_options: profiles.clone(),
            configured_default_profile: Some("profile-20".to_string()),
            effective_default_profile: Some("profile-20".to_string()),
            runtime_default_profile_override: Some("profile-20".to_string()),
            ..UiState::default()
        };
        session_profile.capture_profile_menu_snapshot();
        press_menu_end(&mut session_profile, &snapshot).await;
        let profile_text = render_app_text(80, 24, &mut session_profile, &snapshot);
        for expected in [
            "profile-20",
            "default configured runtime effective",
            "model=gpt-20",
            "reasoning=high",
            "tier=priority",
        ] {
            assert!(
                profile_text.contains(expected),
                "missing {expected:?}\n{profile_text}"
            );
        }

        let mut default_profile = UiState {
            overlay: Overlay::ConfiguredDefaultProfileMenu,
            profile_options: profiles,
            configured_default_profile: Some("profile-20".to_string()),
            effective_default_profile: Some("profile-20".to_string()),
            runtime_default_profile_override: Some("profile-20".to_string()),
            ..UiState::default()
        };
        default_profile.capture_profile_menu_snapshot();
        press_menu_end(&mut default_profile, &snapshot).await;
        let default_text = render_app_text(80, 24, &mut default_profile, &snapshot);
        assert!(default_text.contains("profile-20"), "{default_text}");

        let mut model = UiState {
            overlay: Overlay::SessionModelMenu,
            session_model_options: (1..=20).map(|index| format!("model-{index:02}")).collect(),
            ..UiState::default()
        };
        press_menu_end(&mut model, &snapshot).await;
        let model_text = render_app_text(80, 24, &mut model, &snapshot);
        assert!(model_text.contains("model-20"), "{model_text}");
        assert!(model_text.contains("Custom model"), "{model_text}");
    }

    #[test]
    fn app_smoke_renders_usage_and_providers_at_normal_and_narrow_widths() {
        let snapshot = sample_snapshot();
        let providers = sample_providers();
        let cases = [
            (Page::Stats, 118, 32),
            (Page::Stats, 76, 24),
            (Page::Routing, 118, 32),
            (Page::Routing, 76, 24),
        ];

        for (page, width, height) in cases {
            let mut ui = UiState {
                page,
                focus: Focus::Providers,
                selected_provider_idx: 3,
                selected_stats_provider_idx: 0,
                stats_focus: StatsFocus::Providers,
                language: Language::Zh,
                ..UiState::default()
            };

            let text =
                render_app_text_with_providers(width, height, &mut ui, &snapshot, &providers);

            assert!(text.contains("codex"), "{text}");
            assert!(text.contains("?") || text.contains("帮助"), "{text}");
            let compact_text = text_without_whitespace(&text);
            match page {
                Page::Stats => {
                    assert!(
                        text.contains("Usage") || compact_text.contains("5用量"),
                        "{text}"
                    );
                    assert!(
                        text.contains("Upstream Quota Pool") || compact_text.contains("上游额度池"),
                        "{text}"
                    );
                    if width >= 100 {
                        assert!(compact_text.contains("覆盖范围"), "{text}");
                    }
                }
                Page::Routing => {
                    assert!(text.contains("input-light"), "{text}");
                    assert!(compact_text.contains("路由"), "{text}");
                    assert!(compact_text.contains("候选端点"), "{text}");
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn routing_narrow_layout_keeps_priority_capacity_and_balance_scannable() {
        let snapshot = sample_snapshot();
        let providers = sample_providers();
        let mut ui = UiState {
            page: Page::Routing,
            focus: Focus::Providers,
            selected_provider_idx: 3,
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text_with_providers(76, 22, &mut ui, &snapshot, &providers);

        for expected in [
            "Routing policy",
            "Endpoint candidates",
            "Target",
            "Pri",
            "Cap",
        ] {
            assert!(text.contains(expected), "missing {expected:?}\n{text}");
        }
        assert!(text.contains("input-light"), "{text}");
        assert!(text.contains("3/20"), "{text}");
    }

    #[test]
    fn routing_wide_layout_uses_a_full_height_master_detail_view() {
        let snapshot = sample_snapshot();
        let providers = sample_providers();
        let mut ui = UiState {
            page: Page::Routing,
            focus: Focus::Providers,
            selected_routing_candidate_idx: 3,
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text_with_providers(132, 30, &mut ui, &snapshot, &providers);

        for expected in [
            "Endpoint candidates",
            "Routing policy",
            "Selected endpoint",
            "new-session preference",
            "s prefer; Enter/m state menu",
            "a/Backspace",
            "priority=3",
            "3/20",
            "policy action=provider_cooldown",
            "cooldown=42s",
        ] {
            assert!(text.contains(expected), "missing {expected:?}\n{text}");
        }
        assert!(ui.routing_candidates_visible_rows >= 19);
    }

    #[test]
    fn routing_wide_short_layout_keeps_refresh_status_and_controls_visible() {
        let snapshot = sample_snapshot();
        let providers = sample_providers();

        for height in [22, 24] {
            let mut ui = UiState {
                page: Page::Routing,
                focus: Focus::Providers,
                selected_routing_candidate_idx: 3,
                language: Language::En,
                balance_refresh_in_flight: true,
                ..UiState::default()
            };

            let text = render_app_text_with_providers(132, height, &mut ui, &snapshot, &providers);
            for expected in [
                "balance/quota refresh in progress",
                "Routing controls",
                "s prefer; Enter/m state menu",
                "a/Backspace",
                "force-refresh all balances/quotas",
            ] {
                assert!(
                    text.contains(expected),
                    "height={height}, missing {expected:?}\n{text}"
                );
            }
        }
    }

    #[test]
    fn routing_detail_scroll_reaches_bottom_fields_at_common_terminal_height() {
        let mut snapshot = sample_snapshot();
        snapshot.routing.as_mut().expect("routing").candidates[3].route_path = vec![
            "main".to_string(),
            "regional-relay".to_string(),
            "credential-pool".to_string(),
            "bottom-route-marker".to_string(),
        ];
        let providers = sample_providers();
        let mut ui = UiState {
            page: Page::Routing,
            focus: Focus::Providers,
            selected_routing_candidate_idx: 3,
            routing_detail_focused: true,
            routing_detail_scroll: u16::MAX,
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text_with_providers(120, 24, &mut ui, &snapshot, &providers);

        assert!(text.contains("Routing policy"), "{text}");
        assert!(text.contains("bottom-route-marker"), "{text}");
    }

    #[test]
    fn routing_master_list_scrolls_the_selected_candidate_into_view() {
        let mut snapshot = sample_snapshot();
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
        let mut ui = UiState {
            page: Page::Routing,
            focus: Focus::Providers,
            selected_routing_candidate_idx: 24,
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text_with_providers(132, 30, &mut ui, &snapshot, &[]);

        assert!(text.contains("provider-24"), "{text}");
        assert_eq!(ui.routing_candidates_table.selected(), Some(24));
        assert!(ui.routing_candidates_table.offset() > 0);
    }

    #[test]
    fn routing_tiny_layout_uses_two_lines_without_dropping_priority_or_capacity() {
        let snapshot = sample_snapshot();
        let providers = sample_providers();
        let mut ui = UiState {
            page: Page::Routing,
            focus: Focus::Providers,
            selected_routing_candidate_idx: 3,
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text_with_providers(60, 30, &mut ui, &snapshot, &providers);

        assert!(text.contains("input-li"), "{text}");
        assert!(text.contains("G2"), "{text}");
        assert!(text.contains("Pri 3"), "{text}");
        assert!(text.contains("3/20"), "{text}");
    }

    #[test]
    fn typed_routing_table_renders_at_each_responsive_boundary() {
        let snapshot = sample_snapshot();
        let providers = sample_providers();

        for width in [160, 132, 131, 118, 100, 99, 76, 72, 71, 60] {
            let mut ui = UiState {
                page: Page::Routing,
                focus: Focus::Providers,
                language: Language::En,
                ..UiState::default()
            };
            let text = render_app_text_with_providers(width, 30, &mut ui, &snapshot, &providers);

            assert!(text.contains("Routing policy"), "width={width}\n{text}");
            assert!(
                text.contains("Endpoint candidates"),
                "width={width}\n{text}"
            );
            assert!(text.contains("input"), "width={width}\n{text}");
        }
    }

    #[test]
    fn routing_legacy_snapshot_falls_back_to_read_only_provider_table() {
        let mut snapshot = sample_snapshot();
        snapshot.routing = None;
        let providers = sample_providers();
        let mut ui = UiState {
            page: Page::Routing,
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text_with_providers(100, 24, &mut ui, &snapshot, &providers);

        assert!(text.contains("legacy read-only data"), "{text}");
        assert!(text.contains("input-light"), "{text}");
    }

    #[test]
    fn provider_info_end_scroll_clamps_to_content_and_reaches_the_last_endpoint() {
        let snapshot = sample_snapshot();
        let mut providers = sample_providers();
        let endpoint_template = providers[0].endpoints[0].clone();
        providers[0].endpoints = (0..14)
            .map(|index| UpstreamSummary {
                name: format!("endpoint-{index}"),
                provider_endpoint_key: format!("endpoint:sha256:scroll-{index}"),
                origin: Some(if index == 13 {
                    "https://bottom-endpoint-marker.example.test".to_string()
                } else {
                    format!("https://provider-{index}.example.test")
                }),
                ..endpoint_template.clone()
            })
            .collect();
        providers[0].routable_endpoints = providers[0].endpoints.len();
        let mut ui = UiState {
            page: Page::Routing,
            overlay: Overlay::ProviderInfo,
            selected_provider_idx: 0,
            provider_info_scroll: u16::MAX,
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text_with_providers(100, 24, &mut ui, &snapshot, &providers);

        assert!(ui.provider_info_scroll > 0);
        assert!(ui.provider_info_scroll < u16::MAX);
        assert!(text.contains("bottom-endpoint-marker"), "{text}");
    }

    #[test]
    fn provider_info_and_routing_choose_the_same_latest_equal_rank_balance() {
        let mut snapshot = sample_snapshot();
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![
                ProviderBalanceSnapshot {
                    observation_provider_id: "input-observer".to_string(),
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex", "input", "default",
                    ),
                    fetched_at_ms: 1_000,
                    status: BalanceSnapshotStatus::Ok,
                    plan_name: Some("old-balance-sample".to_string()),
                    total_balance_usd: Some("1".to_string()),
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    observation_provider_id: "input-observer".to_string(),
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex", "input", "default",
                    ),
                    fetched_at_ms: 2_000,
                    status: BalanceSnapshotStatus::Ok,
                    plan_name: Some("latest-balance-sample".to_string()),
                    total_balance_usd: Some("2".to_string()),
                    ..ProviderBalanceSnapshot::default()
                },
            ],
        );
        let providers = sample_providers();
        let mut routing_ui = UiState {
            page: Page::Routing,
            selected_routing_candidate_idx: 0,
            language: Language::En,
            ..UiState::default()
        };
        let routing_text =
            render_app_text_with_providers(132, 30, &mut routing_ui, &snapshot, &providers);
        let mut provider_ui = UiState {
            page: Page::Routing,
            overlay: Overlay::ProviderInfo,
            selected_provider_idx: 0,
            provider_info_endpoint_id: Some("default".to_string()),
            language: Language::En,
            ..UiState::default()
        };
        let provider_text =
            render_app_text_with_providers(120, 36, &mut provider_ui, &snapshot, &providers);

        for (surface, text) in [("routing", routing_text), ("provider info", provider_text)] {
            assert!(
                text.contains("latest-balance-sample"),
                "{surface} did not render the newest equal-rank balance\n{text}"
            );
            assert!(
                !text.contains("old-balance-sample"),
                "{surface} rendered the older equal-rank balance\n{text}"
            );
        }
    }

    #[test]
    fn endpoint_details_surface_upstream_usage_telemetry_and_windows() {
        let mut snapshot = sample_snapshot();
        let balance = snapshot
            .provider_balances
            .get_mut("input-light")
            .and_then(|balances| balances.first_mut())
            .expect("input-light balance");
        balance.source = "usage_provider:sub2api_usage".to_string();
        balance.usage_rate = Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
            average_duration_ms: Some("842.7".to_string()),
            rpm: Some("0.7".to_string()),
            tpm: Some("85.3".to_string()),
        });
        balance.usage_windows = vec![codex_helper_core::balance::ProviderUsageWindow {
            period: "daily".to_string(),
            used_usd: Some("95".to_string()),
            limit_usd: Some("100".to_string()),
            remaining_usd: Some("5".to_string()),
            unlimited: Some(false),
        }];
        let providers = sample_providers();

        let mut routing_ui = UiState {
            page: Page::Routing,
            selected_routing_candidate_idx: 3,
            language: Language::En,
            ..UiState::default()
        };
        let routing_text =
            render_app_text_with_providers(132, 64, &mut routing_ui, &snapshot, &providers);
        let mut provider_ui = UiState {
            page: Page::Routing,
            overlay: Overlay::ProviderInfo,
            selected_provider_idx: 3,
            provider_info_endpoint_id: Some("default".to_string()),
            language: Language::En,
            ..UiState::default()
        };
        let provider_text =
            render_app_text_with_providers(120, 64, &mut provider_ui, &snapshot, &providers);

        for (surface, text) in [("routing", routing_text), ("provider info", provider_text)] {
            for expected in [
                "Upstream usage report",
                "Sub2API usage API",
                "RPM 0.7",
                "daily used $95.00 left $5.00 / $100.00",
            ] {
                assert!(
                    text.contains(expected),
                    "{surface} is missing {expected:?}\n{text}"
                );
            }
        }
    }

    #[test]
    fn endpoint_details_hide_retained_usage_after_refresh_error() {
        let mut snapshot = sample_snapshot();
        let balance = snapshot
            .provider_balances
            .get_mut("input-light")
            .and_then(|balances| balances.first_mut())
            .expect("input-light balance");
        balance.source = "usage_provider:sub2api_usage".to_string();
        balance.status = BalanceSnapshotStatus::Error;
        balance.error = Some("connection failed".to_string());
        balance.usage_rate = Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
            rpm: Some("0.7".to_string()),
            ..Default::default()
        });
        balance.usage_windows = vec![codex_helper_core::balance::ProviderUsageWindow {
            period: "daily".to_string(),
            used_usd: Some("95".to_string()),
            limit_usd: Some("100".to_string()),
            remaining_usd: Some("5".to_string()),
            unlimited: Some(false),
        }];
        let providers = sample_providers();

        let mut routing_ui = UiState {
            page: Page::Routing,
            selected_routing_candidate_idx: 3,
            language: Language::En,
            ..UiState::default()
        };
        let routing_text =
            render_app_text_with_providers(132, 64, &mut routing_ui, &snapshot, &providers);
        let mut provider_ui = UiState {
            page: Page::Routing,
            overlay: Overlay::ProviderInfo,
            selected_provider_idx: 3,
            provider_info_endpoint_id: Some("default".to_string()),
            language: Language::En,
            ..UiState::default()
        };
        let provider_text =
            render_app_text_with_providers(120, 64, &mut provider_ui, &snapshot, &providers);

        for (surface, text) in [("routing", routing_text), ("provider info", provider_text)] {
            assert!(
                !text.contains("Upstream usage report"),
                "{surface} must not present retained telemetry as a fresh report:\n{text}"
            );
        }
    }

    #[test]
    fn endpoint_details_hide_ambiguous_multi_observer_usage_reports() {
        let mut snapshot = sample_snapshot();
        let reports = snapshot
            .provider_balances
            .get_mut("input-light")
            .expect("input-light balances");
        let balance = reports.first_mut().expect("input-light balance");
        balance.source = "usage_provider:sub2api_usage".to_string();
        balance.usage_rate = Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
            rpm: Some("0.7".to_string()),
            ..Default::default()
        });
        let mut competing = balance.clone();
        competing.observation_provider_id = "second-usage-observer".to_string();
        competing.source = "usage_provider:new_api_token_usage".to_string();
        reports.push(competing);

        let providers = sample_providers();
        let mut routing_ui = UiState {
            page: Page::Routing,
            selected_routing_candidate_idx: 3,
            language: Language::En,
            ..UiState::default()
        };
        let routing_text =
            render_app_text_with_providers(132, 64, &mut routing_ui, &snapshot, &providers);
        let mut provider_ui = UiState {
            page: Page::Routing,
            overlay: Overlay::ProviderInfo,
            selected_provider_idx: 3,
            provider_info_endpoint_id: Some("default".to_string()),
            language: Language::En,
            ..UiState::default()
        };
        let provider_text =
            render_app_text_with_providers(120, 64, &mut provider_ui, &snapshot, &providers);

        for (surface, text) in [("routing", routing_text), ("provider info", provider_text)] {
            assert!(
                !text.contains("Upstream usage report"),
                "{surface} must not select one of multiple current upstream reports:\n{text}"
            );
        }
    }

    #[test]
    fn routing_narrow_layout_hides_retained_balance_after_refresh_error() {
        let mut snapshot = sample_snapshot();
        let balance = snapshot
            .provider_balances
            .get_mut("input-light")
            .and_then(|balances| balances.first_mut())
            .expect("input-light balance");
        balance.status = BalanceSnapshotStatus::Error;
        balance.error = Some("connection failed".to_string());
        balance.quota_period = Some("daily".to_string());
        balance.quota_remaining_usd = Some("5".to_string());
        balance.quota_limit_usd = Some("100".to_string());
        let providers = sample_providers();

        for width in [80, 60] {
            let mut ui = UiState {
                page: Page::Routing,
                selected_routing_candidate_idx: 3,
                language: Language::En,
                ..UiState::default()
            };
            let text = render_app_text_with_providers(width, 30, &mut ui, &snapshot, &providers);

            assert!(
                text.contains("error"),
                "missing error at {width} cols:\n{text}"
            );
            assert!(
                !text.contains("$5/$100") && !text.contains("left $5.00"),
                "retained amount leaked at {width} cols:\n{text}"
            );
        }
    }

    #[test]
    fn startup_alert_modal_renders_issue_and_close_hint() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            overlay: Overlay::StartupAlert,
            startup_readiness: Some(sample_startup_readiness()),
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text(88, 28, &mut ui, &snapshot);

        assert!(text.contains("Startup guardrail"), "{text}");
        assert!(text.contains("Codex switch requires recovery"), "{text}");
        assert!(text.contains("Esc/Enter close startup guardrail"), "{text}");
    }

    #[test]
    fn startup_alert_modal_keeps_core_copy_visible_at_narrow_width() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            overlay: Overlay::StartupAlert,
            startup_readiness: Some(sample_startup_readiness()),
            language: Language::En,
            ..UiState::default()
        };

        let text = render_app_text(64, 24, &mut ui, &snapshot);

        assert!(text.contains("Startup guardrail"), "{text}");
        assert!(text.contains("switch requires recovery"), "{text}");
        assert!(text.contains("recorded fingerprints"), "{text}");
        assert!(text.contains("Esc/Enter close"), "{text}");
    }

    #[test]
    fn session_affinity_clear_confirmation_shows_state_bound_risk() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            page: Page::Sessions,
            overlay: Overlay::SessionAffinityConfirmation,
            language: Language::En,
            session_affinity_confirmation: Some(
                crate::proxy::OperatorSessionAffinityMutationRequest {
                    session_key: "session:sha256:test".to_string(),
                    expected_affinity_revision: Some("affinity:v1:test".to_string()),
                    command: crate::proxy::OperatorSessionAffinityCommand::Clear,
                },
            ),
            ..UiState::default()
        };

        let text = render_app_text(104, 30, &mut ui, &snapshot);

        assert!(
            text.contains("state-bound / hard requests may be rejected"),
            "{text}"
        );
        assert!(
            text.contains("next eligible request reruns current")
                && text.contains("routing policy"),
            "{text}"
        );
        assert!(
            text.contains("WebSocket selects another endpoint")
                && text.contains("requires reconnect"),
            "{text}"
        );
    }

    #[test]
    fn session_affinity_actions_keep_selected_candidate_visible_in_long_list() {
        let mut snapshot = sample_snapshot();
        snapshot.routing.as_mut().expect("routing").candidates = (0..41)
            .map(|index| OperatorRouteCandidateSummary {
                route_order: index,
                provider_id: format!("provider-{index}"),
                endpoint_id: "default".to_string(),
                preference_group: (index / 5) as u32,
                route_path: vec!["main".to_string()],
            })
            .collect();
        let mut row = unknown_session_row();
        row.session_id = Some("session:sha256:test".to_string());
        row.active_count = 0;
        row.route_affinity = Some(SessionRouteAffinityView {
            revision: "affinity:v1:test".to_string(),
            provider_id: "provider-0".to_string(),
            endpoint_id: "default".to_string(),
            upstream_origin: "https://provider-0.example.test".to_string(),
            route_path: vec!["main".to_string()],
            last_selected_at_ms: 1,
            last_changed_at_ms: 1,
            change_reason: "selected".to_string(),
        });
        snapshot.rows.push(row);
        let mut ui = UiState {
            page: Page::Sessions,
            overlay: Overlay::SessionAffinityActions,
            language: Language::En,
            session_affinity_action_selected_idx: 38,
            ..UiState::default()
        };

        let text = render_app_text(110, 30, &mut ui, &snapshot);

        assert!(text.contains("Clear affinity"), "{text}");
        assert!(text.contains("Rebind to provider-37.default"), "{text}");
        assert!(text.contains("Candidates"), "{text}");
        assert!(text.contains("of 41"), "{text}");
    }

    #[test]
    fn session_binding_menus_explain_fast_and_render_manual_values() {
        let mut snapshot = sample_snapshot();
        let mut row = unknown_session_row();
        row.session_id = Some("session:sha256:binding".to_string());
        row.active_count = 0;
        row.binding_profile_name = Some("daily".to_string());
        row.binding_continuity_mode = Some(crate::state::SessionContinuityMode::ManualProfile);
        row.binding = crate::state::SessionBindingProjection {
            revision: "binding:v1:test".to_string(),
            profile_name: Some("daily".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            continuity_mode: Some(crate::state::SessionContinuityMode::ManualProfile),
        };
        snapshot.rows.push(row);
        let selected_index = snapshot.rows.len() - 1;
        let mut ui = UiState {
            page: Page::Sessions,
            overlay: Overlay::SessionServiceTierMenu,
            language: Language::En,
            session_service_tier_menu_idx: 2,
            selected_session_idx: selected_index,
            selected_sessions_page_idx: selected_index,
            ..UiState::default()
        };

        let modal = render_app_text(104, 32, &mut ui, &snapshot);
        assert!(modal.contains("fast (upstream priority)"), "{modal}");
        assert!(
            modal.contains("fast is sent upstream as priority"),
            "{modal}"
        );

        ui.overlay = Overlay::None;
        let page = render_app_text(132, 36, &mut ui, &snapshot);
        assert!(
            page.contains("model=gpt-5.4 effort=high tier=priority"),
            "{page}"
        );
        assert!(page.contains("b profile  M model  E effort"), "{page}");
    }

    #[test]
    fn sessions_detail_hints_keep_effort_affinity_and_filter_keys_distinct() {
        let mut snapshot = sample_snapshot();
        let mut row = unknown_session_row();
        row.session_id = Some("session:sha256:key-hints".to_string());
        snapshot.rows.push(row);
        let selected_index = snapshot.rows.len() - 1;
        let mut ui = UiState {
            page: Page::Sessions,
            language: Language::En,
            selected_session_idx: selected_index,
            selected_sessions_page_idx: selected_index,
            sessions_details_scroll: u16::MAX,
            ..UiState::default()
        };

        let text = render_app_text(132, 36, &mut ui, &snapshot);
        let compact_text = text_without_whitespace(&text);

        assert!(compact_text.contains("atoggleactive-only"), "{text}");
        assert!(compact_text.contains("Entereffortmenu"), "{text}");
        assert!(compact_text.contains("p/Asessionrouteactions"), "{text}");
        assert!(
            !compact_text.contains("Enteradvancedaffinityactions"),
            "{text}"
        );
    }

    #[test]
    fn dashboard_renders_unknown_session_activity() {
        let mut snapshot = sample_snapshot();
        snapshot.rows.push(unknown_session_row());
        let mut ui = UiState {
            page: Page::Dashboard,
            language: Language::Zh,
            ..UiState::default()
        };

        let text = render_app_text(96, 28, &mut ui, &snapshot);
        let compact_text = text_without_whitespace(&text);

        assert!(compact_text.contains("未知"), "{text}");
        assert!(text.contains("RUN"), "{text}");
    }

    #[test]
    fn observed_only_session_explains_unavailable_host_directory() {
        let mut snapshot = sample_snapshot();
        snapshot.rows = vec![unknown_session_row()];

        for page in [Page::Dashboard, Page::Sessions] {
            let mut ui = UiState {
                page,
                language: Language::En,
                ..UiState::default()
            };
            let text = render_app_text(132, 40, &mut ui, &snapshot);

            assert!(
                text.contains("unavailable (proxy-observed only)"),
                "{page:?} must explain why the host directory is unavailable:\n{text}"
            );
        }
    }

    #[test]
    fn dashboard_keeps_directory_usage_and_request_cache_visible_at_normal_size() {
        let mut snapshot = sample_snapshot();
        let mut row = unknown_session_row();
        row.session_id = Some("session:sha256:runtime".to_string());
        row.local_session_id = Some("local-session-id".to_string());
        row.cwd = Some("/work/full/project".to_string());
        row.last_usage = Some(crate::usage::UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 200,
            cache_read_input_tokens: 700,
            cache_creation_input_tokens: 50,
            total_tokens: 1_200,
            ..Default::default()
        });
        row.total_usage = row.last_usage.clone();
        snapshot.rows.push(row);
        snapshot.recent.push(
            serde_json::from_value(serde_json::json!({
                "id": 7,
                "session_key": "session:sha256:runtime",
                "usage": {
                    "input_tokens": 1000,
                    "output_tokens": 200,
                    "total_tokens": 1200,
                    "cache_read_input_tokens": 700,
                    "cache_creation_input_tokens": 50
                },
                "observability": {
                    "attempt_count": 1,
                    "route_attempt_count": 0,
                    "retried": false,
                    "cross_provider_failover": false,
                    "same_provider_retry": false,
                    "fast_mode": false,
                    "streaming": true
                },
                "service": "codex",
                "method": "POST",
                "path": "/v1/responses",
                "status_code": 200,
                "duration_ms": 12,
                "streaming": true,
                "ended_at_ms": 1
            }))
            .expect("request fixture"),
        );
        for width in [120, 132, 140] {
            let mut ui = UiState {
                page: Page::Dashboard,
                language: Language::En,
                ..UiState::default()
            };

            let text = render_app_text(width, 40, &mut ui, &snapshot);

            assert!(
                text.lines()
                    .any(|line| line.contains("Sessions") && line.contains("Details")),
                "Dashboard master/detail panels must remain side by side at {width} columns:\n{text}"
            );
            assert!(text.contains("/work/full/project"), "{width}:\n{text}");
            assert!(text.contains("activity:"), "{width}:\n{text}");
            assert!(text.contains("usage:"), "{width}:\n{text}");
            assert!(
                text.lines()
                    .any(|line| ["TTFB", "In", "Out", "Hit%", "CRead", "CNew"]
                        .into_iter()
                        .all(|column| line.contains(column))),
                "Dashboard request metrics must share one visible table header at {width} columns:\n{text}"
            );
            assert!(text.contains("~66.7%"), "{width}:\n{text}");
        }
    }

    #[test]
    fn sessions_keep_usage_columns_visible_at_common_terminal_widths() {
        let mut snapshot = sample_snapshot();
        snapshot.rows.push(unknown_session_row());

        for width in [120, 132, 140] {
            let mut ui = UiState {
                page: Page::Sessions,
                language: Language::En,
                ..UiState::default()
            };

            let text = render_app_text(width, 40, &mut ui, &snapshot);

            assert!(
                text.lines().any(|line| ["turns", "Tok", "tok/s"]
                    .into_iter()
                    .all(|column| line.contains(column))),
                "Sessions usage columns must share one visible table header at {width} columns:\n{text}"
            );
        }
    }

    #[test]
    fn operational_master_detail_pages_stay_side_by_side_at_common_widths() {
        let snapshot = sample_snapshot();
        for width in [120, 132, 140] {
            for (page, master_title, detail_title) in [
                (Page::Sessions, "Sessions", "Session details"),
                (Page::Requests, "Requests", "Details"),
                (Page::ServiceStatus, "service status", "details"),
            ] {
                let mut ui = UiState {
                    page,
                    language: Language::En,
                    ..UiState::default()
                };
                let text = render_app_text(width, 40, &mut ui, &snapshot);

                assert!(
                    text.lines().any(|line| {
                        let line = line.to_ascii_lowercase();
                        line.contains(&master_title.to_ascii_lowercase())
                            && line.contains(&detail_title.to_ascii_lowercase())
                    }),
                    "{page:?} master/detail panels must remain side by side at {width} columns:\n{text}"
                );
            }
        }
    }

    #[test]
    fn operational_master_detail_pages_stack_below_common_widths() {
        let snapshot = sample_snapshot();
        for (page, master_title, detail_title) in [
            (Page::Dashboard, "sessions", "details"),
            (Page::Sessions, "sessions", "session details"),
            (Page::Requests, "requests", "details"),
            (Page::ServiceStatus, "service status", "details"),
        ] {
            let mut ui = UiState {
                page,
                language: Language::En,
                ..UiState::default()
            };
            let text = render_app_text(119, 40, &mut ui, &snapshot);
            let lines = text
                .lines()
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>();
            let master_row = lines
                .iter()
                .position(|line| line.contains(&format!("┌{master_title}")))
                .expect("master panel title");
            let detail_row = lines
                .iter()
                .position(|line| line.contains(&format!("┌{detail_title}")))
                .expect("detail panel title");

            assert!(
                detail_row > master_row,
                "{page:?} must stack below 120 columns:\n{text}"
            );
        }
    }

    #[test]
    fn dashboard_details_scroll_reaches_usage_at_common_terminal_height() {
        let mut snapshot = sample_snapshot();
        let mut row = unknown_session_row();
        row.session_id = Some("session:sha256:scrollable".to_string());
        row.cwd = Some("/work/scrollable/project".to_string());
        row.last_usage = Some(crate::usage::UsageMetrics {
            input_tokens: 900,
            output_tokens: 100,
            cache_read_input_tokens: 600,
            cache_creation_input_tokens: 25,
            total_tokens: 1_000,
            ..Default::default()
        });
        row.total_usage = row.last_usage.clone();
        snapshot.rows.push(row);
        let mut ui = UiState {
            page: Page::Dashboard,
            language: Language::En,
            dashboard_details_scroll: u16::MAX,
            ..UiState::default()
        };

        let text = render_app_text(120, 24, &mut ui, &snapshot);

        assert!(ui.dashboard_details_scroll > 0);
        assert!(ui.dashboard_details_scroll < u16::MAX);
        assert!(text.contains("usage:"), "{text}");
        assert!(text.contains("cache read/create: 600/25"), "{text}");
    }

    #[test]
    fn dashboard_requests_scroll_to_rows_after_the_old_preview_limit() {
        let mut snapshot = sample_snapshot();
        let mut session = unknown_session_row();
        session.session_id = Some("session:sha256:many-requests".to_string());
        snapshot.rows = vec![session];
        snapshot.recent = (1..=75)
            .map(|id| dashboard_request(id, "session:sha256:many-requests"))
            .collect();
        let mut ui = UiState {
            page: Page::Dashboard,
            focus: Focus::Requests,
            selected_session_id: Some("session:sha256:many-requests".to_string()),
            selected_request_idx: 74,
            selected_request_id: Some(75),
            ..UiState::default()
        };
        ui.requests_table.select(Some(74));

        let text = render_app_text(120, 32, &mut ui, &snapshot);

        assert!(
            text.contains("Requests [session:sha256:many-requests]"),
            "{text}"
        );
        assert_eq!(ui.requests_table.selected(), Some(74));
        assert!(ui.requests_table.offset() > 0);
    }

    #[test]
    fn remote_history_stacks_and_scrolls_details_on_narrow_terminals() {
        let snapshot = sample_snapshot();

        for (width, height) in [(76, 24), (60, 20)] {
            let mut ui = UiState {
                page: Page::History,
                language: Language::En,
                runtime_connection: RuntimeConnectionKind::RemoteObserver,
                codex_history_sessions: vec![SessionSummary {
                    id: "history-session-alpha".to_string(),
                    path: "history-session-alpha.jsonl".into(),
                    cwd: Some("/work/project-alpha".to_string()),
                    created_at: Some("2026-07-21T08:00:00Z".to_string()),
                    updated_at: Some("2026-07-21T09:00:00Z".to_string()),
                    last_response_at: Some("2026-07-21T09:00:01Z".to_string()),
                    user_turns: 8,
                    assistant_turns: 8,
                    rounds: 8,
                    first_user_message: Some(
                        "first line\nsecond line\nthird line\nfourth line\nfifth line".to_string(),
                    ),
                    source: SessionSummarySource::LocalFile,
                    sort_hint_ms: Some(1),
                }],
                ..UiState::default()
            };

            let text = render_app_text(width, height, &mut ui, &snapshot);
            assert!(
                text.contains("History sessions (observer-local Codex)"),
                "{text}"
            );
            assert!(text.contains("project-alpha"), "{text}");
            let list_row = text
                .lines()
                .position(|line| line.contains("History sessions"))
                .expect("history title");
            let details_row = text
                .lines()
                .position(|line| line.contains("Details  PgUp/PgDn"))
                .expect("history details title");
            assert!(details_row > list_row, "{width}x{height}\n{text}");

            ui.codex_history_details_scroll = u16::MAX;
            let _ = render_app_text(width, height, &mut ui, &snapshot);
            assert!(ui.codex_history_details_scroll > 0);
            assert!(ui.codex_history_details_scroll < u16::MAX);
        }
    }

    #[test]
    fn history_table_scrolls_to_the_loader_tail_after_external_focus_insertion() {
        let summary = |index: usize| SessionSummary {
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
            source: SessionSummarySource::LocalFile,
            sort_hint_ms: None,
        };
        let mut ui = UiState {
            page: Page::History,
            language: Language::En,
            codex_history_sessions: (0..301).map(summary).collect(),
            selected_codex_history_idx: 300,
            selected_codex_history_id: Some("session-300".to_string()),
            ..UiState::default()
        };
        ui.codex_history_table.select(Some(300));

        let text = render_app_text(120, 32, &mut ui, &sample_snapshot());

        assert!(text.contains("session-300"), "{text}");
        assert_eq!(ui.codex_history_table.selected(), Some(300));
        assert!(ui.codex_history_table.offset() > 0);
    }

    #[test]
    fn remote_recent_stacks_and_scrolls_details_on_narrow_terminals() {
        let snapshot = sample_snapshot();

        for (width, height) in [(76, 24), (60, 20)] {
            let mut ui = UiState {
                page: Page::Recent,
                language: Language::En,
                runtime_connection: RuntimeConnectionKind::RemoteObserver,
                codex_recent_rows: vec![RecentCodexRow {
                    root: "/work/recent-project".to_string(),
                    branch: Some("main".to_string()),
                    session_id: "recent-session-alpha".to_string(),
                    cwd: Some("/work/recent-project/subdirectory".to_string()),
                    mtime_ms: u64::MAX,
                }],
                ..UiState::default()
            };

            let text = render_app_text(width, height, &mut ui, &snapshot);
            assert!(
                text.contains("Recent sessions (observer-local Codex)"),
                "{text}"
            );
            assert!(text.contains("recent-project"), "{text}");
            let list_row = text
                .lines()
                .position(|line| line.contains("Recent sessions"))
                .expect("recent title");
            let details_row = text
                .lines()
                .position(|line| line.contains("Details  PgUp/PgDn"))
                .expect("recent details title");
            assert!(details_row > list_row, "{width}x{height}\n{text}");

            ui.codex_recent_details_scroll = u16::MAX;
            let _ = render_app_text(width, height, &mut ui, &snapshot);
            assert!(ui.codex_recent_details_scroll > 0);
            assert!(ui.codex_recent_details_scroll < u16::MAX);
        }
    }

    #[test]
    fn fleet_page_renders_nodes_work_units_and_details() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            page: Page::Fleet,
            language: Language::En,
            fleet_snapshot: Some(sample_fleet_snapshot()),
            selected_fleet_unit_idx: 1,
            ..UiState::default()
        };

        let text = render_app_text(120, 30, &mut ui, &snapshot);

        assert!(text.contains("0 Fleet"), "{text}");
        assert!(text.contains("local workstation"), "{text}");
        assert!(text.contains("runtime/high"), "{text}");
        assert!(text.contains("session_log/med"), "{text}");
        assert!(text.contains("process/low"), "{text}");
        assert!(text.contains("research fleet behavior"), "{text}");
        assert!(text.contains("waiting_approval"), "{text}");
    }
}
