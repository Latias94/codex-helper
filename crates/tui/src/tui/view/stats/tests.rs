use super::*;
use std::collections::HashMap;
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::dashboard_core::WindowStats;
use crate::state::{
    BalanceSnapshotStatus, FinishedRequest, ProviderBalanceSnapshot, UsageRollupCoverage,
    UsageRollupView,
};
use crate::tui::model::{ForecastBalanceSample, ForecastRecentRequest};
use crate::usage_providers::UsageProviderRefreshSummary;

fn current_test_day() -> i32 {
    (crate::tui::model::now_ms() / 86_400_000) as i32
}

fn sample_snapshot(provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>) -> Snapshot {
    sample_snapshot_with_history(provider_balances, HashMap::new())
}

fn sample_snapshot_with_history(
    provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    provider_balance_history: HashMap<String, Vec<ProviderBalanceSnapshot>>,
) -> Snapshot {
    let day = current_test_day();
    let ok_provider_bucket = UsageBucket {
        requests_total: 4,
        duration_ms_total: 12_000,
        requests_with_usage: 4,
        duration_ms_with_usage_total: 12_000,
        generation_ms_total: 8_000,
        ttfb_ms_total: 600,
        ttfb_samples: 4,
        usage: crate::usage::UsageMetrics {
            input_tokens: 2_000,
            output_tokens: 800,
            total_tokens: 3_000,
            cache_read_input_tokens: 400,
            cache_creation_input_tokens: 100,
            ..Default::default()
        },
        ..UsageBucket::default()
    };
    let stale_provider_bucket = UsageBucket {
        requests_total: 2,
        requests_error: 1,
        duration_ms_total: 6_000,
        requests_with_usage: 2,
        duration_ms_with_usage_total: 6_000,
        generation_ms_total: 4_000,
        usage: crate::usage::UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 500,
            total_tokens: 1_500,
            ..Default::default()
        },
        ..UsageBucket::default()
    };
    let station_bucket = UsageBucket {
        requests_total: 3,
        duration_ms_total: 5_000,
        requests_with_usage: 3,
        duration_ms_with_usage_total: 5_000,
        generation_ms_total: 3_000,
        usage: crate::usage::UsageMetrics {
            input_tokens: 1_200,
            output_tokens: 600,
            total_tokens: 1_800,
            cache_read_input_tokens: 300,
            ..Default::default()
        },
        ..UsageBucket::default()
    };
    let mut today = UsageBucket::default();
    today.add_assign(&ok_provider_bucket);
    today.add_assign(&stale_provider_bucket);
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
        global_route_target_override: None,
        station_meta_overrides: HashMap::new(),
        usage_rollup: UsageRollupView {
            loaded: today.clone(),
            window: today.clone(),
            coverage: UsageRollupCoverage {
                requested_days: 7,
                all_loaded: false,
                loaded_first_day: Some(day),
                loaded_last_day: Some(day),
                loaded_days_with_data: 1,
                loaded_requests: today.requests_total,
                window_first_day: Some(day - 6),
                window_last_day: Some(day),
                window_days_with_data: 1,
                window_requests: today.requests_total,
                window_exceeds_loaded_start: true,
            },
            by_day: vec![(day, today.clone())],
            by_provider: vec![
                ("ok-provider".to_string(), ok_provider_bucket.clone()),
                (
                    "超级中转套餐年度输入提供商".to_string(),
                    stale_provider_bucket.clone(),
                ),
            ],
            by_provider_day: HashMap::from([
                ("ok-provider".to_string(), vec![(day, ok_provider_bucket)]),
                (
                    "超级中转套餐年度输入提供商".to_string(),
                    vec![(day, stale_provider_bucket)],
                ),
            ]),
            by_config: vec![("station".to_string(), station_bucket.clone())],
            by_config_day: HashMap::from([("station".to_string(), vec![(day, station_bucket)])]),
        },
        provider_balances,
        provider_balance_history: provider_balance_history
            .into_iter()
            .map(|(station, snapshots)| {
                (
                    station,
                    snapshots
                        .into_iter()
                        .map(|snapshot| ForecastBalanceSample::from_snapshot(&snapshot))
                        .collect(),
                )
            })
            .collect(),
        station_health: HashMap::new(),
        health_checks: HashMap::new(),
        lb_view: HashMap::new(),
        provider_endpoint_policy_actions: HashMap::new(),
        stats_5m: WindowStats::default(),
        stats_1h: WindowStats::default(),
        service_status: None,
        pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
        refreshed_at: Instant::now(),
    }
}

fn sample_priced_request(ended_at_ms: u64, usd: &str) -> FinishedRequest {
    let usage = crate::usage::UsageMetrics {
        input_tokens: 1_000_000,
        total_tokens: 1_000_000,
        ..Default::default()
    };
    let price = crate::pricing::ModelPrice::from_per_million_usd(
        "gpt-test",
        None,
        usd,
        "0",
        Some("0"),
        Some("0"),
        "test",
    )
    .expect("test price");
    let cost = crate::pricing::estimate_usage_cost_with_accounting(
        &usage,
        &price,
        crate::pricing::CostAdjustments::default(),
        crate::usage::CacheInputAccounting::default(),
    );

    FinishedRequest {
        id: ended_at_ms,
        trace_id: None,
        session_id: None,
        session_identity_source: None,
        client_name: None,
        client_addr: None,
        cwd: None,
        model: Some("gpt-test".to_string()),
        reasoning_effort: None,
        service_tier: None,
        station_name: Some("station".to_string()),
        provider_id: Some("provider".to_string()),
        upstream_base_url: None,
        route_decision: None,
        usage: Some(usage),
        cost,
        retry: None,
        provider_signals: Vec::new(),
        policy_actions: Vec::new(),
        observability: crate::state::RequestObservability::default(),
        service: "codex".to_string(),
        method: "POST".to_string(),
        path: "/v1/responses".to_string(),
        status_code: 200,
        duration_ms: 100,
        ttfb_ms: None,
        streaming: false,
        ended_at_ms,
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
fn stats_kpis_use_brief_refresh_line_for_latest_provider_error() {
    let snapshot = sample_snapshot(HashMap::from([(
        "bad".to_string(),
        vec![ProviderBalanceSnapshot {
            provider_id: "very-long-provider-name-that-would-crowd-the-kpi".to_string(),
            status: BalanceSnapshotStatus::Error,
            error: Some(
                "very long upstream dashboard error text that belongs in provider detail"
                    .to_string(),
            ),
            fetched_at_ms: 100,
            ..ProviderBalanceSnapshot::default()
        }],
    )]));
    let ui = UiState {
        page: crate::tui::types::Page::Stats,
        last_balance_refresh_summary: Some(UsageProviderRefreshSummary {
            attempted: 29,
            refreshed: 28,
            failed: 1,
            ..UsageProviderRefreshSummary::default()
        }),
        ..UiState::default()
    };
    let view = ui.usage_balance_view_for_selection(&snapshot);
    let line = usage_refresh_brief_line(&view, Language::En);

    assert!(line.contains("ok 28/29"), "{line}");
    assert!(line.contains("err very-long-provider-name"), "{line}");
    assert!(!line.contains("latest error"), "{line}");
    assert!(!line.contains("very long upstream"), "{line}");
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
fn stats_render_prioritizes_today_usage_signals() {
    let snapshot = sample_snapshot(HashMap::from([(
        "ok-provider".to_string(),
        vec![ProviderBalanceSnapshot {
            provider_id: "ok-provider".to_string(),
            status: BalanceSnapshotStatus::Ok,
            total_balance_usd: Some("12.50".to_string()),
            ..ProviderBalanceSnapshot::default()
        }],
    )]));
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        stats_focus: StatsFocus::Providers,
        ..UiState::default()
    };

    let text = render_stats_text(140, 30, &mut ui, &snapshot);

    assert!(text.contains("Today usage"), "{text}");
    assert!(text.contains("Today requests"), "{text}");
    assert!(text.contains("Cache / speed"), "{text}");
    assert!(text.contains("Local data coverage"), "{text}");
    assert!(text.contains("Live speed / errors"), "{text}");
    assert!(text.contains("Today providers / stations"), "{text}");
    assert!(text.contains("ok-provider"), "{text}");
    assert!(text.contains("station"), "{text}");
    assert!(text.contains("local history is shorter"), "{text}");
}

#[test]
fn stats_kpis_show_spend_forecast_when_priced_requests_exist() {
    let now = crate::tui::model::now_ms();
    let mut snapshot = sample_snapshot(HashMap::from([(
        "provider".to_string(),
        vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            status: BalanceSnapshotStatus::Ok,
            quota_remaining_usd: Some("20".to_string()),
            fetched_at_ms: now,
            ..ProviderBalanceSnapshot::default()
        }],
    )]));
    snapshot.recent = vec![sample_priced_request(now.saturating_sub(30 * 60_000), "1")];
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        usage_forecast: crate::config::UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 2,
            reset_utc_offset: "+08:00".to_string(),
            ..Default::default()
        },
        ..UiState::default()
    };

    let text = render_stats_text(120, 28, &mut ui, &snapshot);

    assert!(text.contains("burn"), "{text}");
    assert!(text.contains("rate"), "{text}");
    assert!(text.contains("/h"), "{text}");
    assert!(text.contains("low sample"), "{text}");
}

#[test]
fn stats_kpis_show_spend_projection_only_when_sample_is_confident() {
    let now = crate::tui::model::now_ms();
    let mut snapshot = sample_snapshot(HashMap::from([(
        "provider".to_string(),
        vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            status: BalanceSnapshotStatus::Ok,
            quota_remaining_usd: Some("20".to_string()),
            fetched_at_ms: now,
            ..ProviderBalanceSnapshot::default()
        }],
    )]));
    snapshot.recent = vec![
        sample_priced_request(now.saturating_sub(59 * 60_000), "1"),
        sample_priced_request(now.saturating_sub(30 * 60_000), "1"),
    ];
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        usage_forecast: crate::config::UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 2,
            reset_utc_offset: "+08:00".to_string(),
            ..Default::default()
        },
        ..UiState::default()
    };

    let text = render_stats_text(140, 28, &mut ui, &snapshot);

    assert!(text.contains("burn"), "{text}");
    assert!(text.contains("to reset"), "{text}");
    assert!(text.contains("estimated"), "{text}");
}

#[test]
fn stats_kpis_show_package_pacing_for_quota_balances() {
    let now = crate::tui::model::now_ms();
    let mut snapshot = sample_snapshot(HashMap::from([(
        "provider".to_string(),
        vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            status: BalanceSnapshotStatus::Ok,
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("6".to_string()),
            quota_limit_usd: Some("20".to_string()),
            fetched_at_ms: now,
            ..ProviderBalanceSnapshot::default()
        }],
    )]));
    snapshot.recent = vec![
        sample_priced_request(now.saturating_sub(59 * 60_000), "1"),
        sample_priced_request(now.saturating_sub(30 * 60_000), "1"),
    ];
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        usage_forecast: crate::config::UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 2,
            reset_utc_offset: "+08:00".to_string(),
            ..Default::default()
        },
        ..UiState::default()
    };

    let text = render_stats_text(150, 28, &mut ui, &snapshot);

    assert!(text.contains("pace"), "{text}");
    assert!(text.contains("daily package left"), "{text}");
    assert!(text.contains("$6.00/$20.00"), "{text}");
    assert!(text.contains("target"), "{text}");
}

#[test]
fn stats_kpis_marks_balance_calibrated_spend_rate() {
    let now = crate::tui::model::now_ms();
    let current_balance = ProviderBalanceSnapshot {
        provider_id: "provider".to_string(),
        station_name: Some("station".to_string()),
        upstream_index: Some(0),
        status: BalanceSnapshotStatus::Ok,
        quota_period: Some("daily".to_string()),
        quota_remaining_usd: Some("8".to_string()),
        fetched_at_ms: now,
        ..ProviderBalanceSnapshot::default()
    };
    let mut previous_balance = current_balance.clone();
    previous_balance.quota_remaining_usd = Some("10".to_string());
    previous_balance.fetched_at_ms = now.saturating_sub(60 * 60_000);
    let mut snapshot = sample_snapshot_with_history(
        HashMap::from([("station".to_string(), vec![current_balance.clone()])]),
        HashMap::from([(
            "station".to_string(),
            vec![previous_balance, current_balance],
        )]),
    );
    snapshot.recent = vec![sample_priced_request(now.saturating_sub(30 * 60_000), "1")];
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        usage_forecast: crate::config::UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 1,
            reset_utc_offset: "+08:00".to_string(),
            ..Default::default()
        },
        ..UiState::default()
    };

    let text = render_stats_text(150, 28, &mut ui, &snapshot);

    assert!(text.contains("balance-cal 200%"), "{text}");
}

#[test]
fn stats_kpis_label_forecast_sample_sources() {
    let now = crate::tui::model::now_ms();
    let mut snapshot = sample_snapshot(HashMap::new());
    snapshot.recent = vec![
        sample_priced_request(now.saturating_sub(59 * 60_000), "1"),
        sample_priced_request(now.saturating_sub(30 * 60_000), "1"),
        sample_priced_request(now.saturating_sub(10 * 60_000), "1"),
    ];
    snapshot.forecast_recent = snapshot
        .recent
        .iter()
        .map(ForecastRecentRequest::from_finished_request)
        .collect();
    snapshot.forecast_recent_source =
        crate::tui::model::UsageForecastSampleSource::RuntimeAndRequestLedger;
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        usage_forecast: crate::config::UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 2,
            reset_utc_offset: "+08:00".to_string(),
            ..Default::default()
        },
        ..UiState::default()
    };

    let text = render_stats_text(140, 28, &mut ui, &snapshot);

    assert!(text.contains("runtime 3 + local request ledger"), "{text}");
}

#[test]
fn stats_kpis_use_explicit_forecast_sample_source_not_sample_length() {
    let now = crate::tui::model::now_ms();
    let mut snapshot = sample_snapshot(HashMap::new());
    snapshot.recent = vec![
        sample_priced_request(now.saturating_sub(59 * 60_000), "1"),
        sample_priced_request(now.saturating_sub(30 * 60_000), "1"),
    ];
    snapshot.forecast_recent = snapshot
        .recent
        .iter()
        .map(ForecastRecentRequest::from_finished_request)
        .collect();
    snapshot.forecast_recent_source =
        crate::tui::model::UsageForecastSampleSource::RuntimeAndRequestLedger;
    let mut ui = UiState {
        page: crate::tui::types::Page::Stats,
        usage_forecast: crate::config::UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 2,
            reset_utc_offset: "+08:00".to_string(),
            ..Default::default()
        },
        ..UiState::default()
    };

    let text = render_stats_text(140, 28, &mut ui, &snapshot);

    assert!(text.contains("runtime 2 + local request ledger"), "{text}");
}

#[test]
fn spend_forecast_prefers_ledger_backed_sample_over_display_recent() {
    let now = crate::tui::model::now_ms();
    let mut snapshot = sample_snapshot(HashMap::new());
    snapshot.recent = vec![sample_priced_request(now.saturating_sub(30 * 60_000), "1")];
    snapshot.forecast_recent = vec![
        ForecastRecentRequest::from_finished_request(&sample_priced_request(
            now.saturating_sub(59 * 60_000),
            "1",
        )),
        ForecastRecentRequest::from_finished_request(&sample_priced_request(
            now.saturating_sub(30 * 60_000),
            "1",
        )),
        ForecastRecentRequest::from_finished_request(&sample_priced_request(
            now.saturating_sub(10 * 60_000),
            "1",
        )),
    ];
    snapshot.forecast_recent_source =
        crate::tui::model::UsageForecastSampleSource::RuntimeAndRequestLedger;
    let config = crate::config::UsageForecastConfig {
        rate_window_minutes: 60,
        min_priced_requests: 2,
        reset_utc_offset: "+08:00".to_string(),
        ..Default::default()
    };

    let forecast = usage_spend_forecast(&snapshot, &config, now);

    assert_eq!(forecast.priced_requests, 3);
    assert_eq!(
        forecast.confidence,
        crate::usage_forecast::UsageForecastConfidence::Estimated
    );
    assert!(forecast.projected_until_reset_usd.is_some());
}

#[test]
fn spend_forecast_uses_explicit_sample_source_not_forecast_vec_fallback() {
    let now = crate::tui::model::now_ms();
    let mut snapshot = sample_snapshot(HashMap::new());
    snapshot.recent = vec![
        sample_priced_request(now.saturating_sub(30 * 60_000), "1"),
        sample_priced_request(now.saturating_sub(10 * 60_000), "1"),
    ];
    snapshot.forecast_recent = Vec::new();
    snapshot.forecast_recent_source =
        crate::tui::model::UsageForecastSampleSource::RuntimeAndRequestLedger;
    let config = crate::config::UsageForecastConfig {
        rate_window_minutes: 60,
        min_priced_requests: 1,
        reset_utc_offset: "+08:00".to_string(),
        ..Default::default()
    };

    let forecast = usage_spend_forecast(&snapshot, &config, now);

    assert_eq!(forecast.priced_requests, 0);
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
