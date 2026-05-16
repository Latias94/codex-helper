use super::{
    current_page_help_lines, help_text_for_tests, profile_declared_summary,
    profile_resolved_summary,
};
use crate::dashboard_core::ControlProfileOption;
use crate::tui::Language;
use crate::tui::model::Palette;
use crate::tui::types::Page;

fn make_profile(name: &str) -> ControlProfileOption {
    ControlProfileOption {
        name: name.to_string(),
        extends: None,
        station: None,
        model: None,
        reasoning_effort: None,
        service_tier: None,
        fast_mode: false,
        is_default: false,
    }
}

#[test]
fn profile_declared_summary_includes_extends_and_auto_defaults() {
    let mut profile = make_profile("fast");
    profile.extends = Some("base".to_string());
    profile.reasoning_effort = Some("low".to_string());

    let summary = profile_declared_summary(&profile, Language::En);

    assert!(summary.contains("declared:"));
    assert!(summary.contains("extends=base"));
    assert!(summary.contains("reasoning=low"));
    assert!(summary.contains("tier=<auto>"));
}

#[test]
fn profile_resolved_summary_uses_inherited_values() {
    let mut base = make_profile("base");
    base.model = Some("gpt-5.4".to_string());
    base.service_tier = Some("priority".to_string());

    let mut fast = make_profile("fast");
    fast.extends = Some("base".to_string());
    fast.reasoning_effort = Some("low".to_string());

    let (summary, failed) = profile_resolved_summary("fast", &[base, fast], Language::En);

    assert!(!failed);
    assert!(summary.contains("resolved:"));
    assert!(summary.contains("model=gpt-5.4"));
    assert!(summary.contains("reasoning=low"));
    assert!(summary.contains("tier=priority"));
}

#[test]
fn profile_resolved_summary_reports_cycle_error() {
    let mut alpha = make_profile("alpha");
    alpha.extends = Some("beta".to_string());

    let mut beta = make_profile("beta");
    beta.extends = Some("alpha".to_string());

    let (summary, failed) = profile_resolved_summary("alpha", &[alpha, beta], Language::En);

    assert!(failed);
    assert!(summary.contains("resolve failed:"));
    assert!(summary.contains("profile inheritance cycle"));
}

#[test]
fn routing_provider_balance_line_falls_back_for_legacy_snapshot_keys() {
    let snapshot = crate::tui::model::Snapshot {
        rows: Vec::new(),
        recent: Vec::new(),
        model_overrides: std::collections::HashMap::new(),
        overrides: std::collections::HashMap::new(),
        station_overrides: std::collections::HashMap::new(),
        route_target_overrides: std::collections::HashMap::new(),
        service_tier_overrides: std::collections::HashMap::new(),
        global_station_override: None,
        global_route_target_override: None,
        station_meta_overrides: std::collections::HashMap::new(),
        usage_rollup: crate::state::UsageRollupView::default(),
        provider_balances: std::collections::HashMap::from([(
            "input".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                provider_id: String::new(),
                status: crate::state::BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("9.00".to_string()),
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]),
        station_health: std::collections::HashMap::new(),
        health_checks: std::collections::HashMap::new(),
        lb_view: std::collections::HashMap::new(),
        stats_5m: crate::dashboard_core::WindowStats::default(),
        stats_1h: crate::dashboard_core::WindowStats::default(),
        pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
        refreshed_at: std::time::Instant::now(),
    };

    let (_, text) = super::routing_provider_balance_line(&snapshot, "input", Language::En)
        .expect("legacy snapshot should still resolve");

    assert!(text.contains("$9.00"), "{text}");
}

#[test]
fn routing_provider_balance_line_prefers_routing_context() {
    let snapshot = crate::tui::model::Snapshot {
        rows: Vec::new(),
        recent: Vec::new(),
        model_overrides: std::collections::HashMap::new(),
        overrides: std::collections::HashMap::new(),
        station_overrides: std::collections::HashMap::new(),
        route_target_overrides: std::collections::HashMap::new(),
        service_tier_overrides: std::collections::HashMap::new(),
        global_station_override: None,
        global_route_target_override: None,
        station_meta_overrides: std::collections::HashMap::new(),
        usage_rollup: crate::state::UsageRollupView::default(),
        provider_balances: std::collections::HashMap::from([
            (
                "input6".to_string(),
                vec![crate::state::ProviderBalanceSnapshot {
                    provider_id: "input6".to_string(),
                    station_name: Some("input6".to_string()),
                    upstream_index: Some(0),
                    status: crate::state::BalanceSnapshotStatus::Ok,
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
                    status: crate::state::BalanceSnapshotStatus::Exhausted,
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
        station_health: std::collections::HashMap::new(),
        health_checks: std::collections::HashMap::new(),
        lb_view: std::collections::HashMap::new(),
        stats_5m: crate::dashboard_core::WindowStats::default(),
        stats_1h: crate::dashboard_core::WindowStats::default(),
        pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot::default(),
        refreshed_at: std::time::Instant::now(),
    };

    let (balance, text) = super::routing_provider_balance_line(&snapshot, "input6", Language::En)
        .expect("routing snapshot should resolve");

    assert_eq!(balance.station_name.as_deref(), Some("routing"));
    assert!(text.contains("$0") && text.contains("$300.00"), "{text}");
    assert!(!text.contains("$99.00"), "{text}");
}

#[test]
fn current_page_help_includes_hidden_routing_actions() {
    let lines =
        current_page_help_lines(Language::En, Page::Stations, true, true, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("Current page: Routing"), "{text}");
    assert!(text.contains("1/2/0"), "{text}");
    assert!(text.contains("Backspace"), "{text}");
    assert!(text.contains("[]/u/d"), "{text}");
}

#[test]
fn current_page_help_includes_usage_detail_actions() {
    let lines = current_page_help_lines(Language::En, Page::Stats, true, true, Palette::default());
    let text = help_text_for_tests(&lines);

    assert!(text.contains("Current page: Providers"), "{text}");
    assert!(text.contains("PgUp/PgDn"), "{text}");
    assert!(text.contains("refresh provider balances"), "{text}");
}
