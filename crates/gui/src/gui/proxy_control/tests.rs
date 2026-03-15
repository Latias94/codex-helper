use super::*;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::config::{PersistedProviderSpec, PersistedStationSpec, RetryConfig};
use crate::dashboard_core::{ApiV1Snapshot, StationOption, WindowStats};
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, RuntimeConfigState, SessionStats,
    StationHealth, UsageRollupView,
};
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
};
use codex_helper_core::dashboard_core::snapshot::DashboardSnapshot;
use serde_json::Value;
use tokio::task::JoinHandle;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct ScopedEnv {
    saved: Vec<(String, Option<String>)>,
}

impl ScopedEnv {
    unsafe fn set(&mut self, key: &str, value: &str) {
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

fn sample_station(name: &str) -> StationOption {
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

fn sample_snapshot(stations: Vec<StationOption>) -> ApiV1Snapshot {
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
            session_model_overrides: HashMap::new(),
            session_station_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
        },
    }
}

fn spawn_test_server(rt: &tokio::runtime::Runtime, app: Router) -> (String, JoinHandle<()>) {
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

#[test]
fn refresh_attached_prefers_station_snapshot_payload() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
        },
        "host_local_capabilities": {
            "session_history": true,
            "transcript_read": true,
            "cwd_enrichment": true
        },
        "endpoints": [
            "/__codex_helper/api/v1/snapshot",
            "/__codex_helper/api/v1/stations",
            "/__codex_helper/api/v1/stations/runtime"
        ]
    });
    let snapshot = sample_snapshot(vec![sample_station("preferred-station")]);
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/capabilities",
            get({
                let caps = caps.clone();
                move || {
                    let caps = caps.clone();
                    async move { Json(caps) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/snapshot",
            get({
                let snapshot = snapshot.clone();
                move || {
                    let snapshot = snapshot.clone();
                    async move { Json(snapshot) }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4100, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4100, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("attached snapshot");
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "preferred-station");
    assert!(snapshot.shared_capabilities.session_observability);
    assert!(snapshot.shared_capabilities.request_history);
    assert!(snapshot.host_local_capabilities.session_history);
    assert!(snapshot.host_local_capabilities.transcript_read);
    assert!(snapshot.host_local_capabilities.cwd_enrichment);
    assert!(
        controller
            .attached()
            .expect("attached status")
            .supports_station_api
    );

    handle.abort();
}

#[test]
fn refresh_attached_supports_partial_station_surface_with_canonical_effort_api() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": false
        },
        "host_local_capabilities": {
            "session_history": false,
            "transcript_read": true,
            "cwd_enrichment": false
        },
        "remote_admin_access": {
            "loopback_without_token": true,
            "remote_requires_token": false,
            "remote_enabled": false,
            "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
            "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
        },
        "endpoints": [
            "/__codex_helper/api/v1/status/active",
            "/__codex_helper/api/v1/status/recent",
            "/__codex_helper/api/v1/status/session-stats",
            "/__codex_helper/api/v1/status/health-checks",
            "/__codex_helper/api/v1/status/station-health",
            "/__codex_helper/api/v1/runtime/status",
            "/__codex_helper/api/v1/stations",
            "/__codex_helper/api/v1/stations/runtime",
            "/__codex_helper/api/v1/overrides/global-station",
            "/__codex_helper/api/v1/overrides/session/station",
            "/__codex_helper/api/v1/overrides/session/effort",
            "/__codex_helper/api/v1/overrides/session/model"
        ]
    });
    let stations = vec![sample_station("station-partial")];
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/capabilities",
            get({
                let caps = caps.clone();
                move || {
                    let caps = caps.clone();
                    async move { Json(caps) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/status/active",
            get(|| async { Json(Vec::<ActiveRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/recent",
            get(|| async { Json(Vec::<FinishedRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/session-stats",
            get(|| async { Json(HashMap::<String, SessionStats>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/health-checks",
            get(|| async { Json(HashMap::<String, HealthCheckStatus>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/station-health",
            get(|| async { Json(HashMap::<String, StationHealth>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/runtime/status",
            get(|| async {
                Json(serde_json::json!({
                    "loaded_at_ms": 31,
                    "source_mtime_ms": 32,
                }))
            }),
        )
        .route(
            "/__codex_helper/api/v1/stations",
            get({
                let stations = stations.clone();
                move || {
                    let stations = stations.clone();
                    async move { Json(stations) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/global-station",
            get(|| async { Json(Some("station-partial".to_string())) }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/station",
            get(|| async {
                Json(HashMap::from([(
                    "sid-v1".to_string(),
                    "station-partial".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/effort",
            get(|| async {
                Json(HashMap::from([(
                    "sid-v1".to_string(),
                    "medium".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/model",
            get(|| async {
                Json(HashMap::from([(
                    "sid-v1".to_string(),
                    "gpt-5.4".to_string(),
                )]))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4201, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4201, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("partial station snapshot");
    assert!(snapshot.supports_v1);
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "station-partial");
    assert_eq!(
        snapshot.global_station_override.as_deref(),
        Some("station-partial")
    );
    assert_eq!(
        snapshot
            .session_station_overrides
            .get("sid-v1")
            .map(String::as_str),
        Some("station-partial")
    );
    assert_eq!(
        snapshot
            .session_effort_overrides
            .get("sid-v1")
            .map(String::as_str),
        Some("medium")
    );
    assert_eq!(
        snapshot
            .session_model_overrides
            .get("sid-v1")
            .map(String::as_str),
        Some("gpt-5.4")
    );
    assert!(snapshot.session_service_tier_overrides.is_empty());
    assert!(snapshot.shared_capabilities.session_observability);
    assert!(snapshot.host_local_capabilities.transcript_read);
    assert!(!snapshot.supports_default_profile_override);
    assert!(!snapshot.supports_session_override_reset);
    let attached = controller.attached().expect("partial attached status");
    assert_eq!(attached.api_version, Some(1));
    assert_eq!(attached.runtime_loaded_at_ms, Some(31));
    assert_eq!(attached.runtime_source_mtime_ms, Some(32));
    assert!(attached.supports_station_api);
    assert!(attached.supports_station_runtime_override);
    assert!(!attached.remote_admin_access.remote_enabled);
    assert!(!attached.remote_admin_access.remote_requires_token);

    handle.abort();
}

#[test]
fn refresh_attached_prefers_typed_surface_capabilities_over_endpoint_strings() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "surface_capabilities": {
            "status_active": true,
            "status_recent": true,
            "status_session_stats": true,
            "status_health_checks": true,
            "status_station_health": true,
            "runtime_status": true,
            "stations": true,
            "station_runtime": true,
            "global_station_override": true,
            "session_station_override": true,
            "session_reasoning_effort_override": true,
            "session_model_override": true
        },
        "shared_capabilities": {
            "session_observability": true,
            "request_history": false
        },
        "host_local_capabilities": {
            "session_history": false,
            "transcript_read": false,
            "cwd_enrichment": false
        },
        "endpoints": []
    });
    let stations = vec![sample_station("typed-surface")];
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/capabilities",
            get({
                let caps = caps.clone();
                move || {
                    let caps = caps.clone();
                    async move { Json(caps) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/status/active",
            get(|| async { Json(Vec::<ActiveRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/recent",
            get(|| async { Json(Vec::<FinishedRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/session-stats",
            get(|| async { Json(HashMap::<String, SessionStats>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/health-checks",
            get(|| async { Json(HashMap::<String, HealthCheckStatus>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/station-health",
            get(|| async { Json(HashMap::<String, StationHealth>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/runtime/status",
            get(|| async {
                Json(serde_json::json!({
                    "loaded_at_ms": 41,
                    "source_mtime_ms": 42,
                }))
            }),
        )
        .route(
            "/__codex_helper/api/v1/stations",
            get({
                let stations = stations.clone();
                move || {
                    let stations = stations.clone();
                    async move { Json(stations) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/global-station",
            get(|| async { Json(Some("typed-surface".to_string())) }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/station",
            get(|| async {
                Json(HashMap::from([(
                    "sid-typed".to_string(),
                    "typed-surface".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/effort",
            get(|| async {
                Json(HashMap::from([(
                    "sid-typed".to_string(),
                    "high".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/model",
            get(|| async {
                Json(HashMap::from([(
                    "sid-typed".to_string(),
                    "gpt-5.4-fast".to_string(),
                )]))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4202, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4202, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("typed surface snapshot");
    assert!(snapshot.supports_v1);
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "typed-surface");
    assert_eq!(
        snapshot
            .session_station_overrides
            .get("sid-typed")
            .map(String::as_str),
        Some("typed-surface")
    );
    assert_eq!(
        snapshot
            .session_effort_overrides
            .get("sid-typed")
            .map(String::as_str),
        Some("high")
    );
    assert_eq!(
        snapshot
            .session_model_overrides
            .get("sid-typed")
            .map(String::as_str),
        Some("gpt-5.4-fast")
    );
    let attached = controller.attached().expect("typed attached status");
    assert!(attached.supports_station_api);
    assert!(attached.supports_station_runtime_override);
    assert!(!attached.supports_default_profile_override);

    handle.abort();
}

#[test]
fn refresh_attached_rejects_pre_v1_runtime_surface() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let app = Router::new()
        .route(
            "/__codex_helper/status/active",
            get(|| async { Json(Vec::<ActiveRequest>::new()) }),
        )
        .route(
            "/__codex_helper/status/recent",
            get(|| async { Json(Vec::<FinishedRequest>::new()) }),
        )
        .route(
            "/__codex_helper/config/runtime",
            get(|| async {
                Json(serde_json::json!({
                    "loaded_at_ms": 51,
                    "source_mtime_ms": 52,
                }))
            }),
        )
        .route(
            "/__codex_helper/override/session",
            get(|| async {
                Json(HashMap::from([(
                    "sid-legacy".to_string(),
                    "low".to_string(),
                )]))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4202, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4202, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("attached snapshot");
    assert!(!snapshot.supports_v1);
    assert!(snapshot.stations.is_empty());
    assert!(snapshot.session_effort_overrides.is_empty());
    assert!(snapshot.last_error.is_some());
    let attached = controller.attached().expect("attached status");
    assert_eq!(attached.api_version, None);
    assert!(attached.last_error.is_some());
    assert_eq!(attached.runtime_loaded_at_ms, None);
    assert_eq!(attached.runtime_source_mtime_ms, None);
    assert!(!attached.supports_station_api);
    assert!(!attached.supports_station_runtime_override);

    handle.abort();
}

#[test]
fn refresh_attached_prefers_aggregate_session_override_api() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "endpoints": [
            "/__codex_helper/api/v1/status/active",
            "/__codex_helper/api/v1/status/recent",
            "/__codex_helper/api/v1/status/session-stats",
            "/__codex_helper/api/v1/status/health-checks",
            "/__codex_helper/api/v1/status/station-health",
            "/__codex_helper/api/v1/runtime/status",
            "/__codex_helper/api/v1/stations",
            "/__codex_helper/api/v1/overrides/global-station",
            "/__codex_helper/api/v1/overrides/session"
        ]
    });
    let stations = vec![sample_station("aggregate-only")];
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/capabilities",
            get({
                let caps = caps.clone();
                move || {
                    let caps = caps.clone();
                    async move { Json(caps) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/status/active",
            get(|| async { Json(Vec::<ActiveRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/recent",
            get(|| async { Json(Vec::<FinishedRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/session-stats",
            get(|| async { Json(HashMap::<String, SessionStats>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/health-checks",
            get(|| async { Json(HashMap::<String, HealthCheckStatus>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/station-health",
            get(|| async { Json(HashMap::<String, StationHealth>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/runtime/status",
            get(|| async {
                Json(serde_json::json!({
                    "loaded_at_ms": 11,
                    "source_mtime_ms": 22,
                }))
            }),
        )
        .route(
            "/__codex_helper/api/v1/stations",
            get({
                let stations = stations.clone();
                move || {
                    let stations = stations.clone();
                    async move { Json(stations) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/global-station",
            get(|| async { Json(Option::<String>::None) }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session",
            get(|| async {
                Json(serde_json::json!({
                    "sessions": {
                        "sid-a": {
                            "reasoning_effort": "high",
                            "station_name": "aggregate-only",
                            "model": "gpt-5.4",
                            "service_tier": "priority"
                        }
                    }
                }))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4201, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4201, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("aggregate attached snapshot");
    assert_eq!(
        snapshot
            .session_station_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("aggregate-only")
    );
    assert_eq!(
        snapshot
            .session_effort_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("high")
    );
    assert_eq!(
        snapshot
            .session_model_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("gpt-5.4")
    );
    assert_eq!(
        snapshot
            .session_service_tier_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("priority")
    );

    handle.abort();
}

#[test]
fn refresh_attached_sends_admin_token_when_configured() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set(crate::proxy::ADMIN_TOKEN_ENV_VAR, "gui-secret");
    }

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let observed_headers = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
        },
        "host_local_capabilities": {
            "session_history": false,
            "transcript_read": false,
            "cwd_enrichment": false
        },
        "remote_admin_access": {
            "loopback_without_token": true,
            "remote_requires_token": true,
            "remote_enabled": true,
            "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
            "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
        },
        "endpoints": [
            "/__codex_helper/api/v1/snapshot"
        ]
    });
    let snapshot = sample_snapshot(vec![sample_station("alpha")]);
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/capabilities",
            get({
                let caps = caps.clone();
                let observed_headers = observed_headers.clone();
                move |headers: HeaderMap| {
                    let caps = caps.clone();
                    let observed_headers = observed_headers.clone();
                    async move {
                        observed_headers.lock().expect("header lock").push(
                            headers
                                .get(crate::proxy::ADMIN_TOKEN_HEADER)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string),
                        );
                        Json(caps)
                    }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/snapshot",
            get({
                let snapshot = snapshot.clone();
                let observed_headers = observed_headers.clone();
                move |headers: HeaderMap| {
                    let snapshot = snapshot.clone();
                    let observed_headers = observed_headers.clone();
                    async move {
                        observed_headers.lock().expect("header lock").push(
                            headers
                                .get(crate::proxy::ADMIN_TOKEN_HEADER)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string),
                        );
                        Json(snapshot)
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4250, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4250, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let observed_headers = observed_headers.lock().expect("header lock").clone();
    assert!(!observed_headers.is_empty());
    assert!(
        observed_headers
            .iter()
            .all(|value| value.as_deref() == Some("gui-secret"))
    );
    assert!(
        controller
            .attached()
            .expect("attached status")
            .remote_admin_access
            .remote_enabled
    );

    handle.abort();
}

#[test]
fn read_control_trace_entries_prefers_attached_api_when_supported() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let app = Router::new().route(
        "/__codex_helper/api/v1/control-trace",
        get(|| async {
            Json(vec![ControlTraceLogEntry {
                ts_ms: 300,
                kind: "request_completed".to_string(),
                service: Some("codex".to_string()),
                request_id: Some(9),
                event: Some("request_completed".to_string()),
                detail: None,
                payload: serde_json::json!({
                    "method": "POST",
                    "path": "/v1/responses"
                }),
            }])
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4290, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4290);
    attached.admin_base_url = base_url.clone();
    attached.supports_control_trace_api = true;
    controller.mode = ProxyMode::Attached(attached);

    let result = controller
        .read_control_trace_entries(&rt, 80)
        .expect("read attached control trace");

    assert_eq!(
        result.source,
        ControlTraceDataSource::AttachedApi {
            admin_base_url: base_url
        }
    );
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].ts_ms, 300);
    assert_eq!(result.entries[0].kind, "request_completed");

    handle.abort();
}

#[test]
fn read_control_trace_entries_falls_back_to_local_file_when_api_is_unavailable() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let trace_path = std::env::temp_dir()
        .join("codex-helper-gui-tests")
        .join(format!("control-trace-{unique}.jsonl"));
    std::fs::create_dir_all(trace_path.parent().expect("trace parent"))
        .expect("create trace parent");
    unsafe {
        scoped.set(
            "CODEX_HELPER_CONTROL_TRACE_PATH",
            trace_path.to_string_lossy().as_ref(),
        );
    }
    std::fs::write(
        &trace_path,
        [
            serde_json::json!({
                "ts_ms": 100,
                "kind": "retry_trace",
                "service": "codex",
                "request_id": 4,
                "event": "attempt_select",
                "payload": {
                    "event": "attempt_select",
                    "station_name": "right"
                }
            })
            .to_string(),
            serde_json::json!({
                "ts_ms": 200,
                "kind": "request_completed",
                "service": "codex",
                "request_id": 4,
                "event": "request_completed",
                "payload": {
                    "method": "POST",
                    "path": "/v1/responses"
                }
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .expect("write local control trace");

    let mut controller = ProxyController::new(4291, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4291);
    attached.admin_base_url = "http://100.88.0.5:4101".to_string();
    attached.supports_control_trace_api = false;
    controller.mode = ProxyMode::Attached(attached);

    let result = controller
        .read_control_trace_entries(&rt, 80)
        .expect("fallback local control trace");

    assert_eq!(
        result.source,
        ControlTraceDataSource::AttachedFallbackLocal {
            admin_base_url: "http://100.88.0.5:4101".to_string(),
            path: trace_path.clone(),
        }
    );
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].ts_ms, 200);
    assert_eq!(result.entries[0].kind, "request_completed");
}

#[test]
fn attached_runtime_meta_uses_station_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let station_payload = Arc::new(Mutex::new(None::<Value>));
    let station_app = Router::new().route(
        "/__codex_helper/api/v1/stations/runtime",
        post({
            let station_payload = station_payload.clone();
            move |Json(payload): Json<Value>| {
                let station_payload = station_payload.clone();
                async move {
                    *station_payload.lock().expect("station payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (station_base_url, station_handle) = spawn_test_server(&rt, station_app);

    let mut station_controller = ProxyController::new(4300, ServiceKind::Codex);
    let mut station_attached = AttachedStatus::new(4300);
    station_attached.admin_base_url = station_base_url;
    station_attached.supports_station_runtime_override = true;
    station_attached.supports_station_api = true;
    station_controller.mode = ProxyMode::Attached(station_attached);
    station_controller
        .set_runtime_station_meta(
            &rt,
            "alpha".to_string(),
            Some(Some(false)),
            Some(Some(7)),
            Some(Some(RuntimeConfigState::Draining)),
        )
        .expect("station runtime meta update");

    let station_payload = station_payload
        .lock()
        .expect("station payload lock")
        .clone()
        .expect("station payload");
    assert_eq!(
        station_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );
    assert_eq!(
        station_payload.get("runtime_state"),
        Some(&Value::String("draining".to_string()))
    );
    station_handle.abort();
}

#[test]
fn attached_probe_station_uses_station_probe_and_legacy_healthcheck_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let station_payload = Arc::new(Mutex::new(None::<Value>));
    let station_app = Router::new().route(
        "/__codex_helper/api/v1/stations/probe",
        post({
            let station_payload = station_payload.clone();
            move |Json(payload): Json<Value>| {
                let station_payload = station_payload.clone();
                async move {
                    *station_payload.lock().expect("station payload lock") = Some(payload);
                    StatusCode::OK
                }
            }
        }),
    );
    let (station_base_url, station_handle) = spawn_test_server(&rt, station_app);

    let mut station_controller = ProxyController::new(4306, ServiceKind::Codex);
    let mut station_attached = AttachedStatus::new(4306);
    station_attached.admin_base_url = station_base_url;
    station_attached.api_version = Some(1);
    station_attached.supports_station_api = true;
    station_controller.mode = ProxyMode::Attached(station_attached);
    station_controller
        .probe_station(&rt, "alpha".to_string())
        .expect("station probe");

    let station_payload = station_payload
        .lock()
        .expect("station payload lock")
        .clone()
        .expect("station payload");
    assert_eq!(
        station_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );
    station_handle.abort();

    let legacy_payload = Arc::new(Mutex::new(None::<Value>));
    let legacy_app = Router::new().route(
        "/__codex_helper/api/v1/healthcheck/start",
        post({
            let legacy_payload = legacy_payload.clone();
            move |Json(payload): Json<Value>| {
                let legacy_payload = legacy_payload.clone();
                async move {
                    *legacy_payload.lock().expect("legacy payload lock") = Some(payload);
                    StatusCode::OK
                }
            }
        }),
    );
    let (legacy_base_url, legacy_handle) = spawn_test_server(&rt, legacy_app);

    let mut legacy_controller = ProxyController::new(4307, ServiceKind::Codex);
    let mut legacy_attached = AttachedStatus::new(4307);
    legacy_attached.admin_base_url = legacy_base_url;
    legacy_attached.api_version = Some(1);
    legacy_attached.supports_station_api = false;
    legacy_controller.mode = ProxyMode::Attached(legacy_attached);
    legacy_controller
        .probe_station(&rt, "beta".to_string())
        .expect("legacy probe fallback");

    let legacy_payload = legacy_payload
        .lock()
        .expect("legacy payload lock")
        .clone()
        .expect("legacy payload");
    assert_eq!(legacy_payload.get("all"), Some(&Value::Bool(false)));
    assert_eq!(
        legacy_payload
            .get("station_names")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
            .and_then(|value| value.as_str()),
        Some("beta")
    );
    legacy_handle.abort();
}

#[test]
fn attached_persisted_station_config_uses_v1_station_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let active_payload = Arc::new(Mutex::new(None::<Value>));
    let update_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/stations/config-active",
            post({
                let active_payload = active_payload.clone();
                move |Json(payload): Json<Value>| {
                    let active_payload = active_payload.clone();
                    async move {
                        *active_payload.lock().expect("active payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/stations/alpha",
            put({
                let update_payload = update_payload.clone();
                move |Json(payload): Json<Value>| {
                    let update_payload = update_payload.clone();
                    async move {
                        *update_payload.lock().expect("update payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4302, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4302);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_persisted_station_config = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_persisted_active_station(&rt, Some("alpha".to_string()))
        .expect("set persisted active station");
    controller
        .update_persisted_station(&rt, "alpha".to_string(), false, 7)
        .expect("update persisted station");

    let active_payload = active_payload
        .lock()
        .expect("active payload lock")
        .clone()
        .expect("active payload");
    assert_eq!(
        active_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );

    let update_payload = update_payload
        .lock()
        .expect("update payload lock")
        .clone()
        .expect("update payload");
    assert_eq!(update_payload.get("enabled"), Some(&Value::Bool(false)));
    assert_eq!(update_payload.get("level"), Some(&Value::from(7)));

    handle.abort();
}

#[test]
fn attached_persisted_retry_config_uses_v1_retry_endpoint() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__codex_helper/api/v1/retry/config",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("retry payload lock") = Some(payload.clone());
                    Json(serde_json::json!({
                        "configured": payload,
                        "resolved": {
                            "upstream": {
                                "max_attempts": 2,
                                "backoff_ms": 200,
                                "backoff_max_ms": 2000,
                                "jitter_ms": 100,
                                "on_status": "429,500-599,524",
                                "on_class": ["upstream_transport_error"],
                                "strategy": "same_upstream"
                            },
                            "provider": {
                                "max_attempts": 2,
                                "backoff_ms": 0,
                                "backoff_max_ms": 0,
                                "jitter_ms": 0,
                                "on_status": "401,403,404,408,429,500-599,524",
                                "on_class": ["upstream_transport_error"],
                                "strategy": "failover"
                            },
                            "allow_cross_station_before_first_output": true,
                            "never_on_status": "413,415,422",
                            "never_on_class": ["client_error_non_retryable"],
                            "cloudflare_challenge_cooldown_secs": 300,
                            "cloudflare_timeout_cooldown_secs": 12,
                            "transport_cooldown_secs": 45,
                            "cooldown_backoff_factor": 3,
                            "cooldown_backoff_max_secs": 180
                        }
                    }))
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4303, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4303);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_retry_config_api = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_persisted_retry_config(
            &rt,
            RetryConfig {
                profile: Some(crate::config::RetryProfileName::CostPrimary),
                transport_cooldown_secs: Some(45),
                cloudflare_timeout_cooldown_secs: Some(12),
                cooldown_backoff_factor: Some(3),
                cooldown_backoff_max_secs: Some(180),
                ..Default::default()
            },
        )
        .expect("set persisted retry config");

    let observed_payload = observed_payload
        .lock()
        .expect("retry payload lock")
        .clone()
        .expect("retry payload");
    assert_eq!(
        observed_payload.get("profile"),
        Some(&Value::String("cost-primary".to_string()))
    );
    assert_eq!(
        observed_payload.get("transport_cooldown_secs"),
        Some(&Value::from(45))
    );
    assert_eq!(
        observed_payload.get("cooldown_backoff_factor"),
        Some(&Value::from(3))
    );

    let snapshot = controller.snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .configured_retry
            .as_ref()
            .and_then(|retry| retry.profile),
        Some(crate::config::RetryProfileName::CostPrimary)
    );
    assert_eq!(
        snapshot
            .resolved_retry
            .as_ref()
            .map(|retry| retry.transport_cooldown_secs),
        Some(45)
    );
    assert_eq!(
        snapshot
            .resolved_retry
            .as_ref()
            .map(|retry| retry.cooldown_backoff_factor),
        Some(3)
    );
    assert_eq!(
        snapshot
            .resolved_retry
            .as_ref()
            .map(|retry| retry.allow_cross_station_before_first_output),
        Some(true)
    );

    handle.abort();
}

#[test]
fn attached_persisted_station_spec_uses_v1_specs_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
    let delete_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new().route(
        "/__codex_helper/api/v1/stations/specs/alpha",
        put({
            let observed_put_payload = observed_put_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_put_payload = observed_put_payload.clone();
                async move {
                    *observed_put_payload
                        .lock()
                        .expect("station spec payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        })
        .delete({
            let delete_hits = delete_hits.clone();
            move || {
                let delete_hits = delete_hits.clone();
                async move {
                    *delete_hits.lock().expect("delete hits lock") += 1;
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4304, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4304);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_station_spec_api = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .upsert_persisted_station_spec(
            &rt,
            "alpha".to_string(),
            PersistedStationSpec {
                name: "alpha".to_string(),
                alias: Some("Alpha".to_string()),
                enabled: false,
                level: 7,
                members: vec![crate::config::GroupMemberRefV2 {
                    provider: "right".to_string(),
                    endpoint_names: vec!["hk".to_string()],
                    preferred: true,
                }],
            },
        )
        .expect("upsert persisted station spec");
    controller
        .delete_persisted_station_spec(&rt, "alpha".to_string())
        .expect("delete persisted station spec");

    let observed_put_payload = observed_put_payload
        .lock()
        .expect("station spec payload lock")
        .clone()
        .expect("station spec payload");
    assert_eq!(
        observed_put_payload.get("alias"),
        Some(&Value::String("Alpha".to_string()))
    );
    assert_eq!(
        observed_put_payload.get("enabled"),
        Some(&Value::Bool(false))
    );
    assert_eq!(observed_put_payload.get("level"), Some(&Value::from(7)));
    assert_eq!(
        observed_put_payload["members"][0]
            .get("provider")
            .and_then(|value| value.as_str()),
        Some("right")
    );
    assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

    handle.abort();
}

#[test]
fn attached_session_override_reset_uses_v1_reset_endpoint() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__codex_helper/api/v1/overrides/session/reset",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("reset payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4305, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4305);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_session_override_reset = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .clear_session_manual_overrides(&rt, "sid-reset".to_string())
        .expect("reset session manual overrides");

    let observed_payload = observed_payload
        .lock()
        .expect("reset payload lock")
        .clone()
        .expect("reset payload");
    assert_eq!(
        observed_payload.get("session_id"),
        Some(&Value::String("sid-reset".to_string()))
    );

    handle.abort();
}

#[test]
fn attached_session_effort_override_uses_v1_effort_endpoint() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__codex_helper/api/v1/overrides/session/effort",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("effort payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4308, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4308);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .apply_session_effort_override(&rt, "sid-effort".to_string(), Some("high".to_string()))
        .expect("set effort via canonical endpoint");

    let observed_payload = observed_payload
        .lock()
        .expect("effort payload lock")
        .clone()
        .expect("effort payload");
    assert_eq!(
        observed_payload.get("session_id"),
        Some(&Value::String("sid-effort".to_string()))
    );
    assert_eq!(
        observed_payload.get("effort"),
        Some(&Value::String("high".to_string()))
    );

    handle.abort();
}

#[test]
fn attached_persisted_provider_spec_uses_v1_specs_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
    let delete_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new().route(
        "/__codex_helper/api/v1/providers/specs/right",
        put({
            let observed_put_payload = observed_put_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_put_payload = observed_put_payload.clone();
                async move {
                    *observed_put_payload
                        .lock()
                        .expect("provider spec payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        })
        .delete({
            let delete_hits = delete_hits.clone();
            move || {
                let delete_hits = delete_hits.clone();
                async move {
                    *delete_hits.lock().expect("delete hits lock") += 1;
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4305, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4305);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_provider_spec_api = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .upsert_persisted_provider_spec(
            &rt,
            "right".to_string(),
            PersistedProviderSpec {
                name: "right".to_string(),
                alias: Some("Right".to_string()),
                enabled: false,
                auth_token_env: Some("RIGHTCODE_API_KEY".to_string()),
                api_key_env: Some("RIGHTCODE_HEADER_KEY".to_string()),
                endpoints: vec![
                    crate::config::PersistedProviderEndpointSpec {
                        name: "hk".to_string(),
                        base_url: "https://right-hk.example.com/v1".to_string(),
                        enabled: true,
                        priority: 0,
                    },
                    crate::config::PersistedProviderEndpointSpec {
                        name: "us".to_string(),
                        base_url: "https://right-us.example.com/v1".to_string(),
                        enabled: false,
                        priority: 1,
                    },
                ],
            },
        )
        .expect("upsert persisted provider spec");
    controller
        .delete_persisted_provider_spec(&rt, "right".to_string())
        .expect("delete persisted provider spec");

    let observed_put_payload = observed_put_payload
        .lock()
        .expect("provider spec payload lock")
        .clone()
        .expect("provider spec payload");
    assert_eq!(
        observed_put_payload.get("alias"),
        Some(&Value::String("Right".to_string()))
    );
    assert_eq!(
        observed_put_payload.get("enabled"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        observed_put_payload.get("auth_token_env"),
        Some(&Value::String("RIGHTCODE_API_KEY".to_string()))
    );
    assert_eq!(
        observed_put_payload.get("api_key_env"),
        Some(&Value::String("RIGHTCODE_HEADER_KEY".to_string()))
    );
    assert_eq!(
        observed_put_payload["endpoints"][0]
            .get("name")
            .and_then(|value| value.as_str()),
        Some("hk")
    );
    assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

    handle.abort();
}
