use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use super::model::{Palette, ProviderOption, Snapshot};
use super::state::UiState;
use super::types::Overlay;
use crate::tui::i18n::{self, msg};

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
        Overlay::StationInfo => modals::render_station_info_modal(f, p, ui, snapshot, providers),
        Overlay::StartupAlert => modals::render_startup_alert_modal(f, p, ui),
        Overlay::EffortMenu => modals::render_effort_modal(f, p, ui),
        Overlay::ModelMenuSession => modals::render_model_modal(f, p, ui),
        Overlay::ModelInputSession => modals::render_model_input_modal(f, p, ui),
        Overlay::ServiceTierMenuSession => modals::render_service_tier_modal(f, p, ui),
        Overlay::ServiceTierInputSession => modals::render_service_tier_input_modal(f, p, ui),
        Overlay::ProfileMenuSession
        | Overlay::ProfileMenuDefaultRuntime
        | Overlay::ProfileMenuDefaultPersisted => modals::render_profile_modal_v2(f, p, ui),
        Overlay::SessionTranscript => modals::render_session_transcript_modal(f, p, ui),
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            let title = match ui.overlay {
                Overlay::ProviderMenuSession if ui.uses_route_graph_routing() => {
                    i18n::label(ui.language, "session route target")
                }
                Overlay::ProviderMenuSession => {
                    i18n::text(ui.language, msg::OVERLAY_SESSION_PROVIDER_OVERRIDE)
                }
                Overlay::ProviderMenuGlobal if ui.uses_route_graph_routing() => {
                    i18n::label(ui.language, "global route target")
                }
                Overlay::ProviderMenuGlobal => {
                    i18n::text(ui.language, msg::OVERLAY_GLOBAL_STATION_PIN)
                }
                _ => unreachable!(),
            };
            modals::render_provider_modal(f, p, ui, snapshot, providers, title);
        }
        Overlay::RoutingMenu => modals::render_routing_modal(f, p, ui, snapshot),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
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
    use crate::tui::model::SessionRow;
    use crate::tui::model::{RoutingProviderRef, RoutingSpecView, Snapshot};
    use crate::tui::types::{Overlay, Page, StatsFocus};

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
            forecast_recent: Vec::new(),
            forecast_recent_source: crate::tui::model::UsageForecastSampleSource::RuntimeOnly,
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            route_target_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            global_route_target_override: Some("input-light".to_string()),
            station_meta_overrides: HashMap::new(),
            usage_rollup: crate::state::UsageRollupView {
                by_config: vec![(
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
                        provider_id: "input-light".to_string(),
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
                        provider_id: "超级中转套餐年度输入提供商".to_string(),
                        status: BalanceSnapshotStatus::Stale,
                        error: Some("refresh balance failed".to_string()),
                        ..ProviderBalanceSnapshot::default()
                    }],
                ),
            ]),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            refreshed_at: Instant::now(),
        }
    }

    fn sample_routing_spec() -> RoutingSpecView {
        let order = [
            "input",
            "input1",
            "input2",
            "input-light",
            "超级中转套餐年度输入提供商",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();

        RoutingSpecView {
            entry: "main".to_string(),
            routes: BTreeMap::from([(
                "main".to_string(),
                crate::config::RoutingNodeV4 {
                    strategy: crate::config::RoutingPolicyV4::OrderedFailover,
                    children: order.clone(),
                    ..crate::config::RoutingNodeV4::default()
                },
            )]),
            policy: crate::config::RoutingPolicyV4::OrderedFailover,
            order: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            chain: Vec::new(),
            pools: BTreeMap::new(),
            on_exhausted: crate::config::RoutingExhaustedActionV4::Continue,
            entry_strategy: crate::config::RoutingPolicyV4::OrderedFailover,
            expanded_order: order.clone(),
            entry_target: None,
            providers: order
                .iter()
                .map(|name| RoutingProviderRef {
                    name: name.clone(),
                    alias: None,
                    enabled: true,
                    tags: BTreeMap::new(),
                })
                .collect(),
        }
    }

    fn sample_startup_readiness() -> CodexStartupReadiness {
        CodexStartupReadiness {
            issues: vec![CodexStartupReadinessIssue {
                kind: CodexStartupReadinessIssueKind::ClientStateChanged,
                severity: CodexStartupReadinessSeverity::Warning,
                title: "Codex client config changed on startup".to_string(),
                detail: "codex-helper updated ~/.codex/config.toml.".to_string(),
                action: "Restart Codex App before relying on this session.".to_string(),
            }],
        }
    }

    fn remote_control_log_unconfirmed_startup_readiness() -> CodexStartupReadiness {
        CodexStartupReadiness {
            issues: vec![CodexStartupReadinessIssue {
                kind: CodexStartupReadinessIssueKind::RemoteControlLogUnconfirmed,
                severity: CodexStartupReadinessSeverity::Warning,
                title: "Remote-control enablement is not confirmed in Codex logs".to_string(),
                detail: "The config and SQLite state look enabled, but no experimentalFeature/enablement/set success log was found.".to_string(),
                action: "Fully restart Codex App, then run `codex-helper switch remote-control check-logs`.".to_string(),
            }],
        }
    }

    fn unknown_session_row() -> SessionRow {
        SessionRow {
            session_id: None,
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
            last_station_name: None,
            last_upstream_base_url: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: None,
            route_affinity: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
            effective_station: None,
            effective_upstream_base_url: None,
            override_model: None,
            override_effort: None,
            override_station_name: None,
            override_route_target: None,
            override_service_tier: None,
        }
    }

    fn render_app_text(width: u16, height: u16, ui: &mut UiState, snapshot: &Snapshot) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let frame = terminal
            .draw(|frame| {
                render_app(frame, Palette::default(), ui, snapshot, "codex", 18080, &[]);
            })
            .expect("draw");
        buffer_text(frame.buffer)
    }

    #[test]
    fn app_smoke_renders_usage_and_routing_at_normal_and_narrow_widths() {
        let snapshot = sample_snapshot();
        let routing_spec = sample_routing_spec();
        let cases = [
            (Page::Stats, 118, 32),
            (Page::Stats, 76, 24),
            (Page::Stations, 118, 32),
            (Page::Stations, 76, 24),
        ];

        for (page, width, height) in cases {
            let mut ui = UiState {
                page,
                config_version: Some(4),
                routing_spec: Some(routing_spec.clone()),
                selected_station_idx: 3,
                selected_stats_provider_idx: 0,
                stats_focus: StatsFocus::Providers,
                language: Language::Zh,
                ..UiState::default()
            };

            let text = render_app_text(width, height, &mut ui, &snapshot);

            assert!(text.contains("codex"), "{text}");
            assert!(text.contains("?") || text.contains("帮助"), "{text}");
            match page {
                Page::Stats => {
                    assert!(text.contains("Balance") || text.contains("余额"), "{text}");
                    assert!(
                        text.contains("input-light")
                            || (text.contains("超") && text.contains("级")),
                        "{text}"
                    );
                }
                Page::Stations => {
                    assert!(text.contains("entry route main"), "{text}");
                    assert!(text.contains("input-light"), "{text}");
                    assert!(text.contains("ordered-failover"), "{text}");
                }
                _ => unreachable!(),
            }
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
        assert!(
            text.contains("Codex client config changed on startup"),
            "{text}"
        );
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
        assert!(text.contains("config changed"), "{text}");
        assert!(text.contains("Esc/Enter close"), "{text}");
    }

    #[test]
    fn startup_alert_modal_localizes_remote_control_log_warning_in_chinese() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            overlay: Overlay::StartupAlert,
            startup_readiness: Some(remote_control_log_unconfirmed_startup_readiness()),
            language: Language::Zh,
            ..UiState::default()
        };

        let text = render_app_text(96, 28, &mut ui, &snapshot);
        let compact_text = text_without_whitespace(&text);

        assert!(
            compact_text.contains("未在Codex日志中确认远程控制启用"),
            "{text}"
        );
        assert!(compact_text.contains("完整重启CodexApp"), "{text}");
        assert!(compact_text.contains("[警告]"), "{text}");
        assert!(compact_text.contains("下步:"), "{text}");
        assert!(
            !text.contains("Remote-control enablement is not confirmed"),
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
}
