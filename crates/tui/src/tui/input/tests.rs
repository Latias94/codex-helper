use super::{
    BalanceRefreshMode, default_profile_menu_idx, request_provider_balance_refresh,
    routing_entry_children, routing_entry_is_flat_provider_list,
    routing_spec_after_provider_enabled_change, routing_spec_with_order,
    should_request_provider_balance_refresh,
};
use crate::config::{
    ProviderConfigV4, ProxyConfig, ProxyConfigV4, RoutingConfigV4, RoutingExhaustedActionV4,
    RoutingPolicyV4, ServiceConfig, ServiceConfigManager, ServiceViewV4, UpstreamAuth,
};
use crate::dashboard_core::ControlProfileOption;
use crate::lb::LbState;
use crate::proxy::ProxyService;
use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot, ProxyState};
use crate::tui::model::{
    ProviderOption, RoutingProviderRef, RoutingSpecView, Snapshot, routing_provider_names,
};
use crate::tui::state::UiState;
use crate::tui::types::Page;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use tokio::sync::mpsc;

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

fn balance_snapshot(stale: bool, stale_after_ms: Option<u64>) -> ProviderBalanceSnapshot {
    ProviderBalanceSnapshot {
        provider_id: "input".to_string(),
        station_name: Some("input".to_string()),
        upstream_index: Some(0),
        source: "test".to_string(),
        fetched_at_ms: 100,
        stale_after_ms,
        stale,
        exhausted: Some(false),
        status: BalanceSnapshotStatus::Ok,
        ..ProviderBalanceSnapshot::default()
    }
}

fn balance_snapshot_status(
    status: BalanceSnapshotStatus,
    stale: bool,
    stale_after_ms: Option<u64>,
) -> ProviderBalanceSnapshot {
    ProviderBalanceSnapshot {
        status,
        ..balance_snapshot(stale, stale_after_ms)
    }
}

fn balance_map(snapshot: ProviderBalanceSnapshot) -> HashMap<String, Vec<ProviderBalanceSnapshot>> {
    HashMap::from([("input".to_string(), vec![snapshot])])
}

fn stale_routing_explain() -> crate::routing_explain::RoutingExplainResponse {
    crate::routing_explain::RoutingExplainResponse {
        api_version: 1,
        service_name: "codex".to_string(),
        runtime_loaded_at_ms: Some(1),
        request_model: None,
        session_id: None,
        request_context: crate::routing_explain::RoutingExplainRequestContext::default(),
        selected_route: None,
        candidates: Vec::new(),
        affinity_policy: "preferred-group".to_string(),
        affinity: None,
        conditional_routes: Vec::new(),
    }
}

async fn empty_snapshot(state: &ProxyState, cfg: Arc<ProxyConfig>) -> Snapshot {
    crate::tui::model::refresh_snapshot(state, cfg, "codex", 7).await
}

fn proxy_with_single_station_without_upstreams() -> (ProxyService, Arc<ProxyConfig>) {
    let mut codex = ServiceConfigManager {
        active: Some("test".to_string()),
        ..Default::default()
    };
    codex.configs.insert(
        "test".to_string(),
        ServiceConfig {
            name: "test".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: Vec::new(),
        },
    );
    let cfg = Arc::new(ProxyConfig {
        codex,
        ..Default::default()
    });
    let proxy = ProxyService::new(
        reqwest::Client::new(),
        cfg.clone(),
        "codex",
        Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
    );
    (proxy, cfg)
}

struct ScopedEnv {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnv {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn make_temp_home(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    dir.push(format!(
        "codex-helper-tui-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp CODEX_HELPER_HOME");
    dir
}

#[test]
fn auto_balance_refresh_requests_when_cache_is_empty() {
    assert!(should_request_provider_balance_refresh(
        &HashMap::new(),
        BalanceRefreshMode::Auto,
        1_000,
        None
    ));
}

#[test]
fn auto_balance_refresh_reuses_fresh_cache() {
    let balances = balance_map(balance_snapshot(false, Some(2_000)));

    assert!(!should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Auto,
        1_000,
        None
    ));
}

#[test]
fn auto_balance_refresh_requests_when_any_cached_balance_is_stale() {
    let balances = HashMap::from([
        (
            "input".to_string(),
            vec![balance_snapshot(false, Some(2_000))],
        ),
        (
            "backup".to_string(),
            vec![ProviderBalanceSnapshot {
                provider_id: "backup".to_string(),
                station_name: Some("backup".to_string()),
                upstream_index: Some(0),
                source: "test".to_string(),
                fetched_at_ms: 100,
                stale_after_ms: Some(500),
                stale: true,
                ..ProviderBalanceSnapshot::default()
            }],
        ),
    ]);

    assert!(should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Auto,
        1_000,
        None
    ));
    assert!(!should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Auto,
        1_000,
        Some(Duration::from_secs(30))
    ));
    assert!(should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Auto,
        1_000,
        Some(Duration::from_secs(61))
    ));
}

#[test]
fn auto_balance_refresh_requests_for_unknown_or_error_balances() {
    let balances = HashMap::from([
        (
            "input".to_string(),
            vec![balance_snapshot_status(
                BalanceSnapshotStatus::Unknown,
                false,
                Some(2_000),
            )],
        ),
        (
            "backup".to_string(),
            vec![balance_snapshot_status(
                BalanceSnapshotStatus::Error,
                false,
                Some(2_000),
            )],
        ),
    ]);

    assert!(should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Auto,
        1_000,
        None
    ));
    assert!(!should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Auto,
        1_000,
        Some(Duration::from_secs(30))
    ));
    assert!(should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Auto,
        1_000,
        Some(Duration::from_secs(61))
    ));
}

#[test]
fn forced_balance_refresh_bypasses_cache_but_keeps_click_cooldown() {
    let balances = balance_map(balance_snapshot(false, Some(2_000)));

    assert!(should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Force,
        1_000,
        None
    ));
    assert!(!should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Force,
        1_000,
        Some(Duration::from_secs(1))
    ));
    assert!(should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::Force,
        1_000,
        Some(Duration::from_secs(2))
    ));
}

#[test]
fn control_changed_balance_refresh_bypasses_recent_auto_request() {
    let balances = balance_map(balance_snapshot(false, Some(2_000)));

    assert!(should_request_provider_balance_refresh(
        &balances,
        BalanceRefreshMode::ControlChanged,
        1_000,
        Some(Duration::ZERO)
    ));
}

#[tokio::test]
async fn balance_refresh_uses_in_process_proxy_not_admin_http() {
    let temp_home = make_temp_home("balance-refresh-in-process");
    let _scoped_home = ScopedEnv::set_path("CODEX_HELPER_HOME", temp_home.as_path());
    let _persisted = ProxyConfigV4::default();
    std::fs::write(temp_home.join("config.toml"), "version = 5\n")
        .expect("write empty persisted config");

    let (proxy, cfg) = proxy_with_single_station_without_upstreams();
    let mut ui = UiState::default();
    let snapshot = empty_snapshot(proxy.state_handle().as_ref(), cfg).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let started = request_provider_balance_refresh(
        &mut ui,
        &snapshot,
        &proxy,
        BalanceRefreshMode::Force,
        &tx,
    );

    assert!(started);
    let result = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("balance refresh should finish")
        .expect("balance refresh should send outcome");
    assert!(
        result.is_ok(),
        "in-process refresh should not try the invalid admin port: {result:?}"
    );
    assert!(ui.last_balance_refresh_requested_at.is_some());
}

#[tokio::test]
async fn routing_page_g_refreshes_balances() {
    let temp_home = make_temp_home("routing-page-g-refreshes-balances");
    let _scoped_home = ScopedEnv::set_path("CODEX_HELPER_HOME", temp_home.as_path());
    let persisted = ProxyConfigV4 {
        codex: ServiceViewV4 {
            providers: BTreeMap::from([(
                "input".to_string(),
                ProviderConfigV4 {
                    enabled: true,
                    base_url: Some("https://input.example.com/v1".to_string()),
                    inline_auth: UpstreamAuth {
                        auth_token_env: Some("INPUT_KEY".to_string()),
                        ..UpstreamAuth::default()
                    },
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4::ordered_failover(vec!["input".to_string()])),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    crate::config::save_config_v4(&persisted)
        .await
        .expect("write route graph config");
    let loaded = crate::config::load_config_with_v4_source()
        .await
        .expect("load route graph config");
    let proxy = ProxyService::new_with_v4_source(
        reqwest::Client::new(),
        Arc::new(loaded.runtime),
        loaded.v4.map(Arc::new),
        "codex",
        Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
    );
    let mut providers = vec![ProviderOption {
        name: "input".to_string(),
        enabled: true,
        active: true,
        ..ProviderOption::default()
    }];
    let mut ui = UiState {
        page: Page::Stations,
        config_version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
        ..UiState::default()
    };
    let snapshot = empty_snapshot(
        proxy.state_handle().as_ref(),
        Arc::new(ProxyConfig {
            version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
            ..ProxyConfig::default()
        }),
    )
    .await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let handled = super::handle_key_event(
        proxy.state_handle(),
        &mut providers,
        &mut ui,
        &snapshot,
        &proxy,
        tx,
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
    )
    .await;

    assert!(handled);
    assert!(ui.last_balance_refresh_requested_at.is_some());
    let result = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("balance refresh should finish")
        .expect("balance refresh should send outcome");
    assert!(result.is_ok());
}

#[tokio::test]
async fn route_graph_global_route_target_key_uses_routing_order_and_invalidates_preview() {
    let (proxy, cfg) = proxy_with_single_station_without_upstreams();
    let state = ProxyState::new();
    let mut providers = vec![
        ProviderOption {
            name: "input".to_string(),
            enabled: true,
            active: true,
            ..ProviderOption::default()
        },
        ProviderOption {
            name: "backup".to_string(),
            enabled: true,
            active: false,
            ..ProviderOption::default()
        },
    ];
    let snapshot = empty_snapshot(state.as_ref(), cfg).await;
    let mut ui = UiState {
        page: Page::Stations,
        config_version: Some(crate::config::CURRENT_ROUTE_GRAPH_CONFIG_VERSION),
        routing_spec: Some(RoutingSpecView {
            entry: "main".to_string(),
            routes: BTreeMap::new(),
            policy: RoutingPolicyV4::OrderedFailover,
            order: vec!["backup".to_string()],
            target: None,
            prefer_tags: Vec::new(),
            chain: Vec::new(),
            pools: BTreeMap::new(),
            on_exhausted: RoutingExhaustedActionV4::Continue,
            entry_strategy: RoutingPolicyV4::OrderedFailover,
            expanded_order: vec!["backup".to_string(), "input".to_string()],
            entry_target: None,
            providers: vec![
                RoutingProviderRef {
                    name: "input".to_string(),
                    alias: None,
                    enabled: true,
                    tags: BTreeMap::new(),
                },
                RoutingProviderRef {
                    name: "backup".to_string(),
                    alias: None,
                    enabled: true,
                    tags: BTreeMap::new(),
                },
            ],
        }),
        routing_explain: Some(stale_routing_explain()),
        last_routing_control_refresh_at: Some(Instant::now()),
        ..UiState::default()
    };
    let (tx, _rx) = mpsc::unbounded_channel();

    let handled = super::handle_key_event(
        state.clone(),
        &mut providers,
        &mut ui,
        &snapshot,
        &proxy,
        tx,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    )
    .await;

    assert!(handled);
    assert_eq!(
        state.get_global_route_target_override().await.as_deref(),
        Some("backup")
    );
    assert!(ui.routing_explain.is_none());
    assert!(ui.last_routing_control_refresh_at.is_none());
    assert!(ui.needs_snapshot_refresh);
}

#[test]
fn default_profile_menu_idx_offsets_bound_profile_selection() {
    let profiles = vec![make_profile("balanced"), make_profile("fast")];

    assert_eq!(default_profile_menu_idx(&profiles, Some("fast")), 2);
}

#[test]
fn default_profile_menu_idx_falls_back_to_clear_for_missing_binding() {
    let profiles = vec![make_profile("balanced"), make_profile("fast")];

    assert_eq!(default_profile_menu_idx(&profiles, Some("missing")), 0);
}

#[test]
fn default_profile_menu_idx_prefers_first_profile_when_unbound() {
    let profiles = vec![make_profile("balanced"), make_profile("fast")];

    assert_eq!(default_profile_menu_idx(&profiles, None), 1);
    assert_eq!(default_profile_menu_idx(&[], None), 0);
}

#[test]
fn routing_provider_names_appends_missing_catalog_entries() {
    let spec = RoutingSpecView {
        entry: "main".to_string(),
        routes: BTreeMap::new(),
        policy: RoutingPolicyV4::OrderedFailover,
        order: vec!["backup".to_string()],
        target: None,
        prefer_tags: Vec::new(),
        chain: Vec::new(),
        pools: BTreeMap::new(),
        on_exhausted: RoutingExhaustedActionV4::Continue,
        entry_strategy: RoutingPolicyV4::OrderedFailover,
        expanded_order: Vec::new(),
        entry_target: None,
        providers: vec![
            RoutingProviderRef {
                name: "input".to_string(),
                alias: None,
                enabled: true,
                tags: BTreeMap::new(),
            },
            RoutingProviderRef {
                name: "backup".to_string(),
                alias: None,
                enabled: true,
                tags: BTreeMap::new(),
            },
        ],
    };

    assert_eq!(routing_provider_names(&spec), vec!["backup", "input"]);
}

#[test]
fn routing_spec_with_order_clears_target_for_ordered_policy() {
    let spec = RoutingSpecView {
        entry: "main".to_string(),
        routes: BTreeMap::from([(
            "main".to_string(),
            crate::config::RoutingNodeV4 {
                strategy: RoutingPolicyV4::ManualSticky,
                children: vec!["input".to_string()],
                target: Some("input".to_string()),
                prefer_tags: vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                on_exhausted: RoutingExhaustedActionV4::Stop,
                ..crate::config::RoutingNodeV4::default()
            },
        )]),
        policy: RoutingPolicyV4::ManualSticky,
        order: vec!["input".to_string()],
        target: Some("input".to_string()),
        prefer_tags: vec![BTreeMap::from([(
            "billing".to_string(),
            "monthly".to_string(),
        )])],
        chain: Vec::new(),
        pools: BTreeMap::new(),
        on_exhausted: RoutingExhaustedActionV4::Stop,
        entry_strategy: RoutingPolicyV4::ManualSticky,
        expanded_order: Vec::new(),
        entry_target: Some("input".to_string()),
        providers: Vec::new(),
    };

    let next = routing_spec_with_order(
        &spec,
        vec!["backup".to_string(), "input".to_string()],
        RoutingPolicyV4::OrderedFailover,
    );

    assert_eq!(next.policy, RoutingPolicyV4::OrderedFailover);
    assert_eq!(next.target, None);
    assert!(next.prefer_tags.is_empty());
    assert_eq!(next.order, vec!["backup", "input"]);
    assert_eq!(
        next.entry_node().map(|node| node.children.as_slice()),
        Some(&["backup".to_string(), "input".to_string()][..])
    );
}

#[test]
fn disabling_manual_sticky_target_downgrades_to_ordered_failover() {
    let spec = RoutingSpecView {
        entry: "main".to_string(),
        routes: BTreeMap::from([(
            "main".to_string(),
            crate::config::RoutingNodeV4 {
                strategy: RoutingPolicyV4::ManualSticky,
                children: vec!["input".to_string(), "backup".to_string()],
                target: Some("input".to_string()),
                ..crate::config::RoutingNodeV4::default()
            },
        )]),
        policy: RoutingPolicyV4::ManualSticky,
        order: vec!["input".to_string(), "backup".to_string()],
        target: Some("input".to_string()),
        prefer_tags: Vec::new(),
        chain: Vec::new(),
        pools: BTreeMap::new(),
        on_exhausted: RoutingExhaustedActionV4::Continue,
        entry_strategy: RoutingPolicyV4::ManualSticky,
        expanded_order: Vec::new(),
        entry_target: Some("input".to_string()),
        providers: Vec::new(),
    };

    let next = routing_spec_after_provider_enabled_change(&spec, "input", false)
        .expect("manual target disable should rewrite routing");

    assert_eq!(next.policy, RoutingPolicyV4::OrderedFailover);
    assert_eq!(next.target, None);
    assert_eq!(next.order, vec!["input", "backup"]);
}

#[test]
fn enabling_provider_keeps_existing_routing_policy() {
    let spec = RoutingSpecView {
        entry: "main".to_string(),
        routes: BTreeMap::from([(
            "main".to_string(),
            crate::config::RoutingNodeV4 {
                strategy: RoutingPolicyV4::ManualSticky,
                children: vec!["input".to_string()],
                target: Some("input".to_string()),
                ..crate::config::RoutingNodeV4::default()
            },
        )]),
        policy: RoutingPolicyV4::ManualSticky,
        order: vec!["input".to_string()],
        target: Some("input".to_string()),
        prefer_tags: Vec::new(),
        chain: Vec::new(),
        pools: BTreeMap::new(),
        on_exhausted: RoutingExhaustedActionV4::Continue,
        entry_strategy: RoutingPolicyV4::ManualSticky,
        expanded_order: Vec::new(),
        entry_target: Some("input".to_string()),
        providers: Vec::new(),
    };

    assert!(routing_spec_after_provider_enabled_change(&spec, "input", true).is_none());
}

#[test]
fn nested_route_graph_entry_reorder_is_not_flat_provider_list() {
    let spec = RoutingSpecView {
        entry: "monthly_first".to_string(),
        routes: BTreeMap::from([
            (
                "monthly_pool".to_string(),
                crate::config::RoutingNodeV4 {
                    children: vec!["input".to_string(), "input1".to_string()],
                    ..crate::config::RoutingNodeV4::default()
                },
            ),
            (
                "monthly_first".to_string(),
                crate::config::RoutingNodeV4 {
                    children: vec!["monthly_pool".to_string(), "paygo".to_string()],
                    ..crate::config::RoutingNodeV4::default()
                },
            ),
        ]),
        policy: RoutingPolicyV4::OrderedFailover,
        order: vec!["monthly_pool".to_string(), "paygo".to_string()],
        target: None,
        prefer_tags: Vec::new(),
        chain: Vec::new(),
        pools: BTreeMap::new(),
        on_exhausted: RoutingExhaustedActionV4::Continue,
        entry_strategy: RoutingPolicyV4::OrderedFailover,
        expanded_order: vec![
            "input".to_string(),
            "input1".to_string(),
            "paygo".to_string(),
        ],
        entry_target: None,
        providers: vec![
            RoutingProviderRef {
                name: "input".to_string(),
                alias: None,
                enabled: true,
                tags: BTreeMap::new(),
            },
            RoutingProviderRef {
                name: "input1".to_string(),
                alias: None,
                enabled: true,
                tags: BTreeMap::new(),
            },
            RoutingProviderRef {
                name: "paygo".to_string(),
                alias: None,
                enabled: true,
                tags: BTreeMap::new(),
            },
        ],
    };

    assert_eq!(
        routing_entry_children(&spec),
        vec!["monthly_pool".to_string(), "paygo".to_string()]
    );
    assert!(!routing_entry_is_flat_provider_list(&spec));
}
