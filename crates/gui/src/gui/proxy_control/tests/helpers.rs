use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::dashboard_core::{ApiV1Snapshot, StationOption, WindowStats};
use crate::state::{RuntimeConfigState, UsageRollupView};
use axum::Router;
use codex_helper_core::dashboard_core::snapshot::DashboardSnapshot;
use tokio::task::JoinHandle;

pub(super) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
pub(super) struct ScopedEnv {
    saved: Vec<(String, Option<String>)>,
}

impl ScopedEnv {
    pub(super) unsafe fn set(&mut self, key: &str, value: &str) {
        if !self.saved.iter().any(|(saved_key, _)| saved_key == key) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
        }
        unsafe {
            std::env::set_var(key, value);
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        for (key, value) in self.saved.iter().rev() {
            match value {
                Some(value) => unsafe {
                    std::env::set_var(key, value);
                },
                None => unsafe {
                    std::env::remove_var(key);
                },
            }
        }
    }
}

pub(super) fn sample_station(name: &str) -> StationOption {
    StationOption {
        name: name.to_string(),
        alias: None,
        enabled: true,
        level: 1,
        configured_enabled: true,
        configured_level: 1,
        runtime_enabled_override: None,
        runtime_level_override: None,
        runtime_state: RuntimeConfigState::Normal,
        runtime_state_override: None,
        capabilities: Default::default(),
    }
}

pub(super) fn sample_snapshot(stations: Vec<StationOption>) -> ApiV1Snapshot {
    ApiV1Snapshot {
        api_version: 1,
        service_name: "codex".to_string(),
        runtime_loaded_at_ms: Some(1),
        runtime_source_mtime_ms: Some(2),
        stations,
        configured_active_station: None,
        effective_active_station: None,
        default_profile: None,
        profiles: Vec::new(),
        snapshot: DashboardSnapshot {
            refreshed_at_ms: 1,
            active: Vec::new(),
            recent: Vec::new(),
            session_cards: Vec::new(),
            global_station_override: None,
            global_route_target_override: None,
            session_model_overrides: HashMap::new(),
            session_station_overrides: HashMap::new(),
            session_route_target_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            station_health: HashMap::new(),
            provider_balances: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
        },
    }
}

pub(super) fn spawn_test_server(
    rt: &tokio::runtime::Runtime,
    app: Router,
) -> (String, JoinHandle<()>) {
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        (format!("http://{addr}"), handle)
    })
}
