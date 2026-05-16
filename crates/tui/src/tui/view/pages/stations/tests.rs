use super::*;
use std::collections::BTreeMap;
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::state::BalanceSnapshotStatus;
use crate::tui::UpstreamSummary;
use crate::tui::model::station_balance_brief;
use crate::tui::types::Page;

fn provider(
    name: &str,
    enabled: bool,
    level: u8,
    active: bool,
    upstreams: usize,
) -> ProviderOption {
    ProviderOption {
        name: name.to_string(),
        alias: None,
        enabled,
        level,
        active,
        upstreams: (0..upstreams)
            .map(|idx| UpstreamSummary {
                base_url: format!("https://{name}-{idx}.example/v1"),
                ..UpstreamSummary::default()
            })
            .collect(),
    }
}

fn empty_snapshot(
    provider_balances: HashMap<String, Vec<crate::state::ProviderBalanceSnapshot>>,
    global_route_target_override: Option<String>,
) -> Snapshot {
    Snapshot {
        rows: Vec::new(),
        recent: Vec::new(),
        model_overrides: HashMap::new(),
        overrides: HashMap::new(),
        station_overrides: HashMap::new(),
        route_target_overrides: HashMap::new(),
        service_tier_overrides: HashMap::new(),
        global_station_override: None,
        global_route_target_override,
        station_meta_overrides: HashMap::new(),
        usage_rollup: crate::state::UsageRollupView::default(),
        provider_balances,
        station_health: HashMap::new(),
        health_checks: HashMap::new(),
        lb_view: HashMap::new(),
        stats_5m: crate::dashboard_core::WindowStats::default(),
        stats_1h: crate::dashboard_core::WindowStats::default(),
        pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
        refreshed_at: Instant::now(),
    }
}

fn routing_provider(name: &str) -> crate::tui::model::RoutingProviderRef {
    crate::tui::model::RoutingProviderRef {
        name: name.to_string(),
        alias: None,
        enabled: true,
        tags: BTreeMap::new(),
    }
}

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

fn render_stations_text(width: u16, height: u16, ui: &mut UiState, snapshot: &Snapshot) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let frame = terminal
        .draw(|frame| {
            render_stations_page(frame, Palette::default(), ui, snapshot, &[], frame.area());
        })
        .expect("draw");
    buffer_text(frame.buffer)
}

#[test]
fn station_routing_preview_uses_single_level_fallback_order() {
    let providers = vec![
        provider("alpha", true, 1, false, 1),
        provider("beta", true, 1, true, 1),
        provider("disabled", false, 1, false, 1),
    ];
    let lb_view = HashMap::new();

    let provider_balances = HashMap::new();

    let preview = station_routing_posture(
        &providers,
        &HashMap::new(),
        &lb_view,
        &provider_balances,
        None,
        None,
        None,
    );

    assert_eq!(preview.mode, StationRoutingMode::AutoSingleLevelFallback);
    assert_eq!(preview.eligible_candidates[0].name, "beta");
    assert_eq!(preview.eligible_candidates[1].name, "alpha");
    assert_eq!(preview.skipped[0].station_name, "disabled");
    assert_eq!(
        preview.skipped[0].reasons,
        vec![StationRoutingSkipReason::Disabled]
    );
}

#[test]
fn station_routing_preview_sorts_multi_level_and_active_tiebreak() {
    let providers = vec![
        provider("alpha", true, 2, false, 1),
        provider("beta", true, 1, false, 1),
        provider("zeta", true, 2, true, 1),
    ];
    let lb_view = HashMap::new();

    let provider_balances = HashMap::new();

    let preview = station_routing_posture(
        &providers,
        &HashMap::new(),
        &lb_view,
        &provider_balances,
        None,
        None,
        None,
    );

    assert_eq!(preview.mode, StationRoutingMode::AutoLevelFallback);
    assert_eq!(preview.eligible_candidates[0].name, "beta");
    assert_eq!(preview.eligible_candidates[1].name, "zeta");
    assert_eq!(preview.eligible_candidates[2].name, "alpha");
}

#[test]
fn station_routing_preview_applies_runtime_meta_overrides() {
    let providers = vec![
        provider("alpha", true, 3, false, 1),
        provider("beta", true, 3, false, 1),
    ];
    let overrides = HashMap::from([
        ("alpha".to_string(), (Some(false), Some(1))),
        ("beta".to_string(), (None, Some(2))),
    ]);
    let lb_view = HashMap::new();

    let provider_balances = HashMap::new();

    let preview = station_routing_posture(
        &providers,
        &overrides,
        &lb_view,
        &provider_balances,
        None,
        None,
        None,
    );

    assert_eq!(preview.eligible_candidates[0].name, "beta");
    assert_eq!(preview.eligible_candidates[0].level, 2);
    assert_eq!(preview.skipped[0].station_name, "alpha");
    assert_eq!(
        preview.skipped[0].reasons,
        vec![StationRoutingSkipReason::Disabled]
    );
}

#[test]
fn station_routing_preview_marks_pinned_targets() {
    let providers = vec![provider("alpha", false, 1, false, 0)];
    let lb_view = HashMap::new();
    let provider_balances = HashMap::new();

    let preview = station_routing_posture(
        &providers,
        &HashMap::new(),
        &lb_view,
        &provider_balances,
        Some("alpha"),
        None,
        None,
    );

    assert_eq!(preview.mode, StationRoutingMode::PinnedStation);
    assert!(matches!(
        preview.source,
        StationRoutingSource::SessionPin(ref station) if station == "alpha"
    ));
    assert!(preview.eligible_candidates.is_empty());
    assert_eq!(
        preview.skipped[0].reasons,
        vec![StationRoutingSkipReason::NoRoutableUpstreams]
    );
}

#[test]
fn station_routing_preview_marks_balance_warnings() {
    let providers = vec![provider("alpha", true, 1, true, 1)];
    let lb_view = HashMap::new();
    let provider_balances = HashMap::from([(
        "alpha".to_string(),
        vec![crate::state::ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Exhausted,
            ..crate::state::ProviderBalanceSnapshot::default()
        }],
    )]);

    let preview = station_routing_posture(
        &providers,
        &HashMap::new(),
        &lb_view,
        &provider_balances,
        None,
        None,
        None,
    );
    let label = format_routing_candidate(&preview.eligible_candidates[0]);

    assert!(label.contains("balance=exhausted_all"));
}

#[test]
fn station_routing_preview_marks_ignored_routing_exhaustion() {
    let providers = vec![provider("alpha", true, 1, true, 1)];
    let lb_view = HashMap::new();
    let provider_balances = HashMap::from([(
        "alpha".to_string(),
        vec![crate::state::ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Exhausted,
            exhaustion_affects_routing: false,
            ..crate::state::ProviderBalanceSnapshot::default()
        }],
    )]);

    let preview = station_routing_posture(
        &providers,
        &HashMap::new(),
        &lb_view,
        &provider_balances,
        None,
        None,
        None,
    );
    let label = format_routing_candidate(&preview.eligible_candidates[0]);

    assert!(label.contains("balance=exhausted_untrusted"));
    assert!(label.contains("ignored_for_routing=1"));
}

#[test]
fn station_routing_preview_does_not_treat_unknown_balance_as_ok() {
    let providers = vec![provider("alpha", true, 1, true, 1)];
    let lb_view = HashMap::new();
    let provider_balances = HashMap::from([(
        "alpha".to_string(),
        vec![crate::state::ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Unknown,
            ..crate::state::ProviderBalanceSnapshot::default()
        }],
    )]);

    let preview = station_routing_posture(
        &providers,
        &HashMap::new(),
        &lb_view,
        &provider_balances,
        None,
        None,
        None,
    );
    let label = format_routing_candidate(&preview.eligible_candidates[0]);

    assert!(label.contains("balance=unknown=1"));
    assert!(!label.contains("balance=ok"));
}

#[test]
fn routing_order_hint_explains_balance_demotion() {
    let text = format_routing_order_hint(StationRoutingMode::AutoLevelFallback);

    assert!(text.contains("demoted by default"));
    assert!(text.contains("provider-level exceptions"));
}

#[test]
fn route_graph_tree_text_lines_show_nested_routes_and_missing_refs() {
    let spec = crate::tui::model::RoutingSpecView {
        entry: "main".to_string(),
        routes: BTreeMap::from([
            (
                "main".to_string(),
                crate::config::RoutingNodeV4 {
                    strategy: crate::config::RoutingPolicyV4::OrderedFailover,
                    children: vec!["monthly_pool".to_string(), "missing_provider".to_string()],
                    ..crate::config::RoutingNodeV4::default()
                },
            ),
            (
                "monthly_pool".to_string(),
                crate::config::RoutingNodeV4 {
                    strategy: crate::config::RoutingPolicyV4::TagPreferred,
                    children: vec!["monthly_a".to_string(), "paygo_b".to_string()],
                    prefer_tags: vec![BTreeMap::from([(
                        "billing".to_string(),
                        "monthly".to_string(),
                    )])],
                    ..crate::config::RoutingNodeV4::default()
                },
            ),
        ]),
        policy: crate::config::RoutingPolicyV4::OrderedFailover,
        order: Vec::new(),
        target: None,
        prefer_tags: Vec::new(),
        chain: Vec::new(),
        pools: BTreeMap::new(),
        on_exhausted: crate::config::RoutingExhaustedActionV4::Continue,
        entry_strategy: crate::config::RoutingPolicyV4::OrderedFailover,
        expanded_order: Vec::new(),
        entry_target: None,
        providers: vec![
            crate::tui::model::RoutingProviderRef {
                name: "monthly_a".to_string(),
                alias: None,
                enabled: true,
                tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
            },
            crate::tui::model::RoutingProviderRef {
                name: "paygo_b".to_string(),
                alias: None,
                enabled: false,
                tags: BTreeMap::new(),
            },
        ],
    };

    let lines = route_graph_tree_text_lines(&spec, Language::En);

    assert!(lines.iter().any(|line| line.contains("entry route main")));
    assert!(lines.iter().any(|line| line.contains("route monthly_pool")));
    assert!(lines.iter().any(|line| line.contains("provider monthly_a")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("provider paygo_b [off"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("missing ref missing_provider"))
    );
}

#[test]
fn station_balance_brief_shows_single_amount() {
    let provider_balances = HashMap::from([(
        "alpha".to_string(),
        vec![crate::state::ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Ok,
            total_balance_usd: Some("3.50".to_string()),
            ..crate::state::ProviderBalanceSnapshot::default()
        }],
    )]);

    assert_eq!(
        station_balance_brief(&provider_balances, "alpha", 18),
        "left $3.50"
    );
}

#[test]
fn routing_provider_balance_brief_preserves_subscription_amount_in_narrow_table() {
    let snapshot = Snapshot {
        rows: Vec::new(),
        recent: Vec::new(),
        model_overrides: HashMap::new(),
        overrides: HashMap::new(),
        station_overrides: HashMap::new(),
        route_target_overrides: HashMap::new(),
        service_tier_overrides: HashMap::new(),
        global_station_override: None,
        global_route_target_override: None,
        station_meta_overrides: HashMap::new(),
        usage_rollup: crate::state::UsageRollupView::default(),
        provider_balances: HashMap::from([(
            "input".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                provider_id: "input".to_string(),
                status: BalanceSnapshotStatus::Ok,
                plan_name: Some("CodeX Pro Annual".to_string()),
                subscription_balance_usd: Some("165.08".to_string()),
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]),
        station_health: HashMap::new(),
        health_checks: HashMap::new(),
        lb_view: HashMap::new(),
        stats_5m: crate::dashboard_core::WindowStats::default(),
        stats_1h: crate::dashboard_core::WindowStats::default(),
        pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
        refreshed_at: std::time::Instant::now(),
    };

    let brief = routing_provider_balance_brief_lang(
        &snapshot,
        "input",
        usize::from(ROUTING_BALANCE_COLUMN_WIDTH),
        Language::En,
    );

    assert!(brief.contains("$165.08"), "{brief}");
    assert!(!brief.contains('…'), "{brief}");
}

#[test]
fn routing_provider_balance_brief_fits_lazy_quota_in_zh_table_cell() {
    let snapshot = Snapshot {
        rows: Vec::new(),
        recent: Vec::new(),
        model_overrides: HashMap::new(),
        overrides: HashMap::new(),
        station_overrides: HashMap::new(),
        route_target_overrides: HashMap::new(),
        service_tier_overrides: HashMap::new(),
        global_station_override: None,
        global_route_target_override: None,
        station_meta_overrides: HashMap::new(),
        usage_rollup: crate::state::UsageRollupView::default(),
        provider_balances: HashMap::from([(
            "input".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                provider_id: "input".to_string(),
                status: BalanceSnapshotStatus::Exhausted,
                exhausted: Some(true),
                exhaustion_affects_routing: false,
                quota_period: Some("daily".to_string()),
                quota_remaining_usd: Some("0".to_string()),
                quota_limit_usd: Some("300".to_string()),
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]),
        station_health: HashMap::new(),
        health_checks: HashMap::new(),
        lb_view: HashMap::new(),
        stats_5m: crate::dashboard_core::WindowStats::default(),
        stats_1h: crate::dashboard_core::WindowStats::default(),
        pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
        refreshed_at: std::time::Instant::now(),
    };

    let brief = routing_provider_balance_brief_lang(
        &snapshot,
        "input",
        usize::from(ROUTING_BALANCE_COLUMN_WIDTH),
        Language::Zh,
    );

    assert!(
        unicode_width::UnicodeWidthStr::width(brief.as_str())
            <= usize::from(ROUTING_BALANCE_COLUMN_WIDTH),
        "{brief}"
    );
    assert_eq!(brief, "不降级 daily $0/$300.00");
    assert!(!brief.ends_with(" / $"), "{brief}");
}

#[test]
fn routing_provider_balance_prefers_routing_context_over_same_named_station() {
    let snapshot = empty_snapshot(
        HashMap::from([
            (
                "input6".to_string(),
                vec![crate::state::ProviderBalanceSnapshot {
                    provider_id: "input6".to_string(),
                    station_name: Some("input6".to_string()),
                    upstream_index: Some(0),
                    status: BalanceSnapshotStatus::Ok,
                    total_balance_usd: Some("99.00".to_string()),
                    fetched_at_ms: 2_000,
                    ..crate::state::ProviderBalanceSnapshot::default()
                }],
            ),
            (
                "routing".to_string(),
                vec![crate::state::ProviderBalanceSnapshot {
                    provider_id: "input6".to_string(),
                    station_name: Some("routing".to_string()),
                    upstream_index: Some(6),
                    status: BalanceSnapshotStatus::Exhausted,
                    exhausted: Some(true),
                    exhaustion_affects_routing: false,
                    quota_period: Some("daily".to_string()),
                    quota_remaining_usd: Some("0".to_string()),
                    quota_limit_usd: Some("300".to_string()),
                    fetched_at_ms: 1_000,
                    ..crate::state::ProviderBalanceSnapshot::default()
                }],
            ),
        ]),
        None,
    );

    let balances = routing_provider_balance_snapshots(&snapshot, "input6");
    assert_eq!(
        balances
            .first()
            .and_then(|balance| balance.station_name.as_deref()),
        Some("routing")
    );

    let brief = routing_provider_balance_brief_lang(&snapshot, "input6", 80, Language::En);

    assert!(brief.contains("$0") && brief.contains("$300.00"), "{brief}");
    assert!(!brief.contains("$99.00"), "{brief}");
}

#[test]
fn wrapped_route_order_keeps_provider_names_intact() {
    let mut lines = Vec::new();
    let order = [
        "input",
        "input1",
        "input2",
        "input3",
        "input4",
        "input-light",
        "centos",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();

    push_wrapped_segments(&mut lines, Palette::default(), "order", &order, " > ", 36);

    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("input4"), "{text}");
    assert!(text.contains("input-light"), "{text}");
    assert!(!text.contains('…'), "{text}");
}

#[test]
fn folded_route_order_keeps_selected_provider_visible() {
    let order = [
        "input",
        "input1",
        "input2",
        "input3",
        "input4",
        "input-light",
        "centos",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();

    let folded = folded_route_chain_segments(&order, Some("input-light"), 6);
    let text = folded.join(" > ");

    assert!(text.contains("*input-light"), "{text}");
    assert!(text.contains("input"), "{text}");
    assert!(text.contains("centos"), "{text}");
    assert!(text.contains("... +"), "{text}");
    assert!(!text.contains("inp…ght"), "{text}");
}

#[test]
fn route_graph_routing_render_folds_long_order_and_keeps_target_balance_visible() {
    let order = [
        "input",
        "input1",
        "input2",
        "input3",
        "input4",
        "input-light",
        "centos",
        "超级中转套餐年度输入提供商",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let spec = crate::tui::model::RoutingSpecView {
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
        providers: order.iter().map(|name| routing_provider(name)).collect(),
    };
    let snapshot = empty_snapshot(
        HashMap::from([(
            "input-light".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                provider_id: "input-light".to_string(),
                status: BalanceSnapshotStatus::Exhausted,
                exhausted: Some(true),
                exhaustion_affects_routing: false,
                quota_period: Some("daily".to_string()),
                quota_remaining_usd: Some("0".to_string()),
                quota_limit_usd: Some("300".to_string()),
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]),
        Some("input-light".to_string()),
    );
    let mut ui = UiState {
        page: Page::Stations,
        config_version: Some(5),
        routing_spec: Some(spec),
        selected_station_idx: 5,
        language: Language::Zh,
        ..UiState::default()
    };

    let text = render_stations_text(84, 28, &mut ui, &snapshot);

    assert!(text.contains("provider") && text.contains("#6/8"), "{text}");
    assert!(text.contains("*input-light"), "{text}");
    assert!(text.contains("$0/$300.00"), "{text}");
    assert!(
        text.contains("不") && text.contains("降") && text.contains("级"),
        "{text}"
    );
    assert!(text.contains("超") && text.contains("级"), "{text}");
    assert!(!text.contains("inp…ght"), "{text}");
    assert!(!text.contains("$0/$│"), "{text}");
}

#[test]
fn station_balance_brief_prefers_usable_snapshot_and_keeps_warning() {
    let provider_balances = HashMap::from([(
        "alpha".to_string(),
        vec![
            crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Exhausted,
                ..crate::state::ProviderBalanceSnapshot::default()
            },
            crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("1.00".to_string()),
                ..crate::state::ProviderBalanceSnapshot::default()
            },
        ],
    )]);

    assert_eq!(
        station_balance_brief(&provider_balances, "alpha", 18),
        "left $1.00 exh 1"
    );
}
