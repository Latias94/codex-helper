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

    use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot, UsageBucket};
    use crate::tui::Language;
    use crate::tui::model::{RoutingProviderRef, RoutingSpecView, Snapshot};
    use crate::tui::types::{Page, StatsFocus};

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

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
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
}
