use super::*;
use std::collections::HashMap;
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::state::UsageRollupView;
use crate::usage_providers::UsageProviderRefreshSummary;

fn sample_snapshot(provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>) -> Snapshot {
    Snapshot {
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
        usage_rollup: UsageRollupView {
            by_provider: vec![
                (
                    "ok-provider".to_string(),
                    UsageBucket {
                        requests_total: 1,
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
            ..UsageRollupView::default()
        },
        provider_balances,
        station_health: HashMap::new(),
        health_checks: HashMap::new(),
        lb_view: HashMap::new(),
        stats_5m: WindowStats::default(),
        stats_1h: WindowStats::default(),
        pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
        refreshed_at: Instant::now(),
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

fn render_stats_text(width: u16, height: u16, ui: &mut UiState, snapshot: &Snapshot) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let frame = terminal
        .draw(|frame| {
            render_stats_page(frame, Palette::default(), ui, snapshot, &[], frame.area());
        })
        .expect("draw");
    buffer_text(frame.buffer)
}

#[test]
fn stats_attention_filter_keeps_balance_and_error_rows() {
    let snapshot = sample_snapshot(HashMap::from([
        (
            "ok-provider".to_string(),
            vec![ProviderBalanceSnapshot {
                provider_id: "ok-provider".to_string(),
                status: BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("12.50".to_string()),
                ..ProviderBalanceSnapshot::default()
            }],
        ),
        (
            "input".to_string(),
            vec![ProviderBalanceSnapshot {
                provider_id: "超级中转套餐年度输入提供商".to_string(),
                status: BalanceSnapshotStatus::Stale,
                error: Some("refresh balance failed".to_string()),
                ..ProviderBalanceSnapshot::default()
            }],
        ),
    ]));
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        stats_attention_only: true,
        ..UiState::default()
    };
    let view = ui.usage_balance_view_for_selection(&snapshot);

    let rows = ui.filtered_usage_balance_provider_rows(&view);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].provider_id, "超级中转套餐年度输入提供商");
    ui.stats_attention_only = false;
    assert_eq!(ui.filtered_usage_balance_provider_rows(&view).len(), 2);
}

#[test]
fn stats_refresh_line_shows_summary_counts_and_latest_provider_error() {
    let snapshot = sample_snapshot(HashMap::from([(
        "bad".to_string(),
        vec![ProviderBalanceSnapshot {
            provider_id: "bad-provider".to_string(),
            status: BalanceSnapshotStatus::Error,
            error: Some("lookup failed".to_string()),
            fetched_at_ms: 100,
            ..ProviderBalanceSnapshot::default()
        }],
    )]));
    let ui = UiState {
        page: crate::tui::types::Page::Stats,
        last_balance_refresh_summary: Some(UsageProviderRefreshSummary {
            attempted: 4,
            refreshed: 3,
            failed: 1,
            missing_token: 1,
            auto_attempted: 2,
            auto_refreshed: 1,
            ..UsageProviderRefreshSummary::default()
        }),
        ..UiState::default()
    };
    let view = ui.usage_balance_view_for_selection(&snapshot);
    let line = usage_refresh_line(&view, Language::En);

    assert!(line.contains("ok 3/4"), "{line}");
    assert!(line.contains("failed 1"), "{line}");
    assert!(line.contains("missing key 1"), "{line}");
    assert!(line.contains("bad-provider"), "{line}");
    assert!(line.contains("lookup failed"), "{line}");
    assert!(line.contains("latest error"), "{line}");
}

#[test]
fn stats_narrow_render_keeps_cjk_provider_and_complete_balance_amount() {
    let snapshot = sample_snapshot(HashMap::from([
        (
            "ok-provider".to_string(),
            vec![ProviderBalanceSnapshot {
                provider_id: "ok-provider".to_string(),
                status: BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("12.50".to_string()),
                ..ProviderBalanceSnapshot::default()
            }],
        ),
        (
            "input".to_string(),
            vec![ProviderBalanceSnapshot {
                provider_id: "超级中转套餐年度输入提供商".to_string(),
                status: BalanceSnapshotStatus::Exhausted,
                exhausted: Some(true),
                exhaustion_affects_routing: false,
                quota_period: Some("daily".to_string()),
                quota_remaining_usd: Some("0".to_string()),
                quota_limit_usd: Some("300".to_string()),
                ..ProviderBalanceSnapshot::default()
            }],
        ),
    ]));
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        stats_focus: StatsFocus::Providers,
        stats_attention_only: true,
        language: Language::Zh,
        ..UiState::default()
    };

    let text = render_stats_text(84, 28, &mut ui, &snapshot);

    assert!(text.contains("超") && text.contains("级"), "{text}");
    assert!(text.contains("$0/$300.00"), "{text}");
}

#[test]
fn provider_detail_scrolls_endpoint_rows_independently() {
    let balances = (0..8)
        .map(|idx| ProviderBalanceSnapshot {
            provider_id: "scroll-provider".to_string(),
            upstream_index: Some(idx),
            status: BalanceSnapshotStatus::Ok,
            total_balance_usd: Some(format!("{}", 100 - idx)),
            ..ProviderBalanceSnapshot::default()
        })
        .collect::<Vec<_>>();
    let snapshot = sample_snapshot(HashMap::from([("scroll".to_string(), balances)]));
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        stats_focus: StatsFocus::Providers,
        stats_provider_detail_scroll: 3,
        ..UiState::default()
    };
    let view = ui.usage_balance_view_for_selection(&snapshot);
    ui.selected_stats_provider_idx = view
        .provider_rows
        .iter()
        .position(|row| row.provider_id == "scroll-provider")
        .expect("provider row");
    let row = ui
        .selected_usage_balance_provider_row(&view)
        .expect("selected provider row");
    let backend = TestBackend::new(100, 16);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let frame = terminal
        .draw(|frame| {
            render_provider_usage_detail(
                frame,
                Palette::default(),
                &mut ui,
                &view,
                Some(row),
                "7d",
                frame.area(),
                Language::En,
            );
        })
        .expect("draw");

    let text = buffer_text(frame.buffer);
    assert!(text.contains("upstream#3"), "{text}");
    assert!(text.contains("upstream#6"), "{text}");
    assert!(!text.contains("upstream#0"), "{text}");
    assert_eq!(ui.stats_provider_detail_scroll, 3);
}
