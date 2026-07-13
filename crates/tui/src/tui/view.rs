use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use super::model::{Palette, ProviderOption, Snapshot};
use super::state::UiState;
use super::types::Overlay;

mod chrome;
mod modals;
mod pages;
mod provider_control;
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

    use crate::codex_integration::{
        CodexStartupReadiness, CodexStartupReadinessIssue, CodexStartupReadinessIssueKind,
        CodexStartupReadinessSeverity,
    };
    use crate::state::{
        BalanceSnapshotStatus, ProviderBalanceSnapshot, SessionObservationScope, UsageBucket,
    };
    use crate::tui::Language;
    use crate::tui::model::{SessionRow, Snapshot, UpstreamSummary};
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
            endpoints: vec![UpstreamSummary {
                provider_name: name.to_string(),
                name: "default".to_string(),
                provider_endpoint_key: format!("endpoint:sha256:{idx}"),
                origin: Some(format!("https://provider-{idx}.example.test")),
                priority: idx as u32,
                configured_enabled: true,
                effective_enabled: true,
                routable: true,
                runtime_enabled_override: None,
                runtime_state: Default::default(),
                runtime_state_override: None,
                capacity: Default::default(),
                policy_actions: Vec::new(),
            }],
            capacity: Default::default(),
        })
        .collect()
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
                        text.contains("Remote Quota") || compact_text.contains("远端额度"),
                        "{text}"
                    );
                    if width >= 100 {
                        assert!(compact_text.contains("覆盖范围"), "{text}");
                    }
                }
                Page::Routing => {
                    assert!(text.contains("input-light"), "{text}");
                    assert!(compact_text.contains("路由"), "{text}");
                    assert!(compact_text.contains("提供商"), "{text}");
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn routing_narrow_layout_keeps_provider_table_and_details_scannable() {
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
            "Providers",
            "Name",
            "Cfg",
            "Eff",
            "Routable",
            "Balance/Quota",
            "Provider details: input-light",
            "Balance / quota",
            "Endpoints",
        ] {
            assert!(text.contains(expected), "missing {expected:?}\n{text}");
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
