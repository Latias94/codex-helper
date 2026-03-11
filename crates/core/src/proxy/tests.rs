use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};

use axum::Json;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::http::{HeaderValue, Request};
use axum::response::Response;
use axum::routing::post;
use futures_util::stream;
use reqwest::Client;
use tokio::time::{Duration, sleep};
use tower::util::ServiceExt;

use crate::config::{
    ProxyConfig, RetryConfig, RetryProfileName, RetryStrategy, ServiceConfig, ServiceConfigManager,
    ServiceControlProfile, UiConfig, UpstreamAuth, UpstreamConfig,
};
use crate::proxy::ProxyService;
use crate::state::RuntimeConfigState;

fn spawn_axum_server(app: axum::Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    (addr, handle)
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
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

    unsafe fn set_path(&mut self, key: &str, value: &Path) {
        unsafe {
            self.set(key, value.to_string_lossy().as_ref());
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

fn make_temp_test_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("codex-helper-proxy-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn write_text_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, content).expect("write test file");
}

fn make_proxy_config(upstreams: Vec<UpstreamConfig>, retry: RetryConfig) -> ProxyConfig {
    let mut mgr = ServiceConfigManager {
        active: Some("test".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "test".to_string(),
        ServiceConfig {
            name: "test".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams,
        },
    );

    ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry,
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    }
}

fn reserve_unused_local_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("local_addr")
}

async fn send_responses_request(
    client: &Client,
    proxy_addr: std::net::SocketAddr,
    session_id: Option<&str>,
) -> reqwest::Response {
    let mut request = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"input":"hi"}"#);
    if let Some(session_id) = session_id {
        request = request.header("session_id", session_id);
    }
    request.send().await.expect("send request")
}

async fn send_responses_json(
    client: &Client,
    proxy_addr: std::net::SocketAddr,
    session_id: Option<&str>,
) -> serde_json::Value {
    send_responses_request(client, proxy_addr, session_id)
        .await
        .error_for_status()
        .expect("request status")
        .json::<serde_json::Value>()
        .await
        .expect("request json")
}

#[tokio::test]
async fn proxy_api_v1_capabilities_and_overrides_work() {
    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut t = HashMap::new();
                t.insert("provider_id".to_string(), "u1".to_string());
                t
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.codex.default_profile = Some("fast".to_string());
    cfg.codex.profiles.insert(
        "fast".to_string(),
        ServiceControlProfile {
            station: Some("test".to_string()),
            model: Some("gpt-5.4-mini".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
        },
    );
    cfg.codex.profiles.insert(
        "steady".to_string(),
        ServiceControlProfile {
            station: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
        },
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let caps = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/capabilities",
            proxy_addr
        ))
        .send()
        .await
        .expect("caps send")
        .error_for_status()
        .expect("caps status")
        .json::<serde_json::Value>()
        .await
        .expect("caps json");
    assert_eq!(caps.get("api_version").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        caps.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/profiles"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/configs/runtime"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/runtime"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/profiles/default"))
    }));
    let host_local_history = crate::config::codex_sessions_dir().is_dir();
    assert_eq!(
        caps["shared_capabilities"]["session_observability"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["shared_capabilities"]["request_history"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["host_local_capabilities"]["session_history"].as_bool(),
        Some(host_local_history)
    );
    assert_eq!(
        caps["host_local_capabilities"]["transcript_read"].as_bool(),
        Some(host_local_history)
    );
    assert_eq!(
        caps["host_local_capabilities"]["cwd_enrichment"].as_bool(),
        Some(host_local_history)
    );

    let set_global = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/global-config",
            proxy_addr
        ))
        .json(&serde_json::json!({ "config_name": "test" }))
        .send()
        .await
        .expect("set global send");
    assert_eq!(set_global.status(), StatusCode::NO_CONTENT);

    let global = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/global-config",
            proxy_addr
        ))
        .send()
        .await
        .expect("get global send")
        .error_for_status()
        .expect("get global status")
        .json::<serde_json::Value>()
        .await
        .expect("get global json");
    assert_eq!(global.as_str(), Some("test"));

    let set_effort = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/effort",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1", "effort": "high" }))
        .send()
        .await
        .expect("set effort send");
    assert_eq!(set_effort.status(), StatusCode::NO_CONTENT);

    let set_session_cfg = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/config",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1", "config_name": "test" }))
        .send()
        .await
        .expect("set session config send");
    assert_eq!(set_session_cfg.status(), StatusCode::NO_CONTENT);

    let effort_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/effort",
            proxy_addr
        ))
        .send()
        .await
        .expect("get effort send")
        .error_for_status()
        .expect("get effort status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get effort json");
    assert_eq!(effort_map.get("s1").map(String::as_str), Some("high"));

    let session_cfg_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/config",
            proxy_addr
        ))
        .send()
        .await
        .expect("get session config send")
        .error_for_status()
        .expect("get session config status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get session config json");
    assert_eq!(session_cfg_map.get("s1").map(String::as_str), Some("test"));

    let profiles = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/profiles",
            proxy_addr
        ))
        .send()
        .await
        .expect("get profiles send")
        .error_for_status()
        .expect("get profiles status")
        .json::<serde_json::Value>()
        .await
        .expect("get profiles json");
    assert_eq!(
        profiles.get("default_profile").and_then(|v| v.as_str()),
        Some("fast")
    );
    assert_eq!(
        profiles["profiles"][0]
            .get("service_tier")
            .and_then(|v| v.as_str()),
        Some("priority")
    );

    let set_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "steady" }))
        .send()
        .await
        .expect("set default profile send");
    assert_eq!(set_default.status(), StatusCode::NO_CONTENT);

    let profiles = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/profiles",
            proxy_addr
        ))
        .send()
        .await
        .expect("get profiles after override send")
        .error_for_status()
        .expect("get profiles after override status")
        .json::<serde_json::Value>()
        .await
        .expect("get profiles after override json");
    assert_eq!(
        profiles.get("default_profile").and_then(|v| v.as_str()),
        Some("steady")
    );

    let clear_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": null }))
        .send()
        .await
        .expect("clear default profile send");
    assert_eq!(clear_default.status(), StatusCode::NO_CONTENT);

    let profiles = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/profiles",
            proxy_addr
        ))
        .send()
        .await
        .expect("get profiles after clear send")
        .error_for_status()
        .expect("get profiles after clear status")
        .json::<serde_json::Value>()
        .await
        .expect("get profiles after clear json");
    assert_eq!(
        profiles.get("default_profile").and_then(|v| v.as_str()),
        Some("fast")
    );

    let apply_profile = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/profile",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s2", "profile_name": "fast" }))
        .send()
        .await
        .expect("apply profile send");
    assert_eq!(apply_profile.status(), StatusCode::NO_CONTENT);

    let model_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/model",
            proxy_addr
        ))
        .send()
        .await
        .expect("get model send")
        .error_for_status()
        .expect("get model status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get model json");
    assert!(!model_map.contains_key("s2"));

    let tier_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/service-tier",
            proxy_addr
        ))
        .send()
        .await
        .expect("get tier send")
        .error_for_status()
        .expect("get tier status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get tier json");
    assert!(!tier_map.contains_key("s2"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_rejects_incompatible_profile_station_capabilities() {
    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::from([
                ("supports_fast_mode".to_string(), "false".to_string()),
                ("supports_reasoning".to_string(), "false".to_string()),
            ]),
            supported_models: HashMap::from([("gpt-5.4".to_string(), true)]),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.codex.profiles.insert(
        "strict".to_string(),
        ServiceControlProfile {
            station: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
        },
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let set_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "strict" }))
        .send()
        .await
        .expect("set incompatible default profile send");
    assert_eq!(set_default.status(), StatusCode::BAD_REQUEST);
    let set_default_body = set_default.text().await.expect("set default body");
    assert!(set_default_body.contains("service_tier"));

    let apply_profile = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/profile",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "session_id": "sid-incompatible-profile",
            "profile_name": "strict",
        }))
        .send()
        .await
        .expect("apply incompatible profile send");
    assert_eq!(apply_profile.status(), StatusCode::BAD_REQUEST);
    let apply_profile_body = apply_profile.text().await.expect("apply profile body");
    assert!(apply_profile_body.contains("service_tier"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_capability_mismatch_fails_over_without_poisoning_health() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_hits_for_route = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_hits = primary_hits_for_route.clone();
            async move {
                primary_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": {
                            "type": "unsupported_value",
                            "message": "service_tier 'priority' is not supported by this provider"
                        }
                    })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_hits_for_route = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_hits = backup_hits_for_route.clone();
            async move {
                backup_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "upstream": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let mut mgr = ServiceConfigManager {
        active: Some("primary".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", primary_addr),
                auth: UpstreamAuth::default(),
                tags: HashMap::from([("provider_id".to_string(), "primary".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: None,
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", backup_addr),
                auth: UpstreamAuth::default(),
                tags: HashMap::from([("provider_id".to_string(), "backup".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let proxy_for_state = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"input":"hi","service_tier":"priority"}"#)
        .send()
        .await
        .expect("send capability mismatch request")
        .error_for_status()
        .expect("capability mismatch final status")
        .json::<serde_json::Value>()
        .await
        .expect("capability mismatch final json");
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let lb_view = proxy_for_state.state.get_lb_view().await;
    let primary_lb = lb_view.get("primary").expect("primary lb view");
    assert_eq!(primary_lb.upstreams.len(), 1);
    assert_eq!(primary_lb.upstreams[0].failure_count, 0);
    assert_eq!(primary_lb.upstreams[0].cooldown_remaining_secs, None);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_snapshot_works() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut t = HashMap::new();
                t.insert("provider_id".to_string(), "u1".to_string());
                t
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::from([("gpt-5.4".to_string(), "gpt-5.4-fast".to_string())]),
        }],
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    proxy
        .state
        .set_session_config_override("sid-1".to_string(), "test".to_string(), 1)
        .await;
    proxy
        .state
        .set_session_service_tier_override("sid-1".to_string(), "priority".to_string(), 1)
        .await;
    let req_id = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-1".to_string()),
            Some("G:/codes/demo".to_string()),
            Some("gpt-5.4".to_string()),
            Some("medium".to_string()),
            Some("priority".to_string()),
            1,
        )
        .await;
    proxy
        .state
        .update_request_route(
            req_id,
            "test".to_string(),
            Some("u1".to_string()),
            "http://127.0.0.1:9/v1".to_string(),
        )
        .await;

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let snap = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot?recent_limit=10&stats_days=7",
            proxy_addr
        ))
        .send()
        .await
        .expect("snapshot send")
        .error_for_status()
        .expect("snapshot status")
        .json::<serde_json::Value>()
        .await
        .expect("snapshot json");

    assert_eq!(snap.get("api_version").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        snap.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );
    assert!(
        snap.get("snapshot").is_some(),
        "should include snapshot object"
    );
    assert!(snap.get("configs").is_some(), "should include configs list");
    assert!(
        snap.get("stations").is_some(),
        "should include stations list"
    );
    assert_eq!(snap.get("configs"), snap.get("stations"));
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_config_name"]["source"].as_str(),
        Some("session_override")
    );
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_model"]["value"].as_str(),
        Some("gpt-5.4-fast")
    );
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_model"]["source"].as_str(),
        Some("station_mapping")
    );
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_service_tier"]["source"].as_str(),
        Some("session_override")
    );

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_default_profile_binding_applies_to_new_session() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|body: Bytes| async move {
            let json: serde_json::Value =
                serde_json::from_slice(&body).expect("echo upstream json");
            (StatusCode::OK, Json(json))
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", upstream_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut t = HashMap::new();
                t.insert("provider_id".to_string(), "u-bind".to_string());
                t
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.codex.default_profile = Some("daily".to_string());
    cfg.codex.profiles.insert(
        "daily".to_string(),
        ServiceControlProfile {
            station: Some("test".to_string()),
            model: Some("gpt-5.4-fast".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
        },
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("session_id", "sid-bind")
        .body(r#"{"input":"hi"}"#)
        .send()
        .await
        .expect("send bind request")
        .error_for_status()
        .expect("bind request status")
        .json::<serde_json::Value>()
        .await
        .expect("bind request json");

    assert_eq!(
        resp.get("model").and_then(|v| v.as_str()),
        Some("gpt-5.4-fast")
    );
    assert_eq!(
        resp.get("service_tier").and_then(|v| v.as_str()),
        Some("priority")
    );
    assert_eq!(
        resp.get("reasoning")
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str()),
        Some("low")
    );

    let snap = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot?recent_limit=10&stats_days=7",
            proxy_addr
        ))
        .send()
        .await
        .expect("binding snapshot send")
        .error_for_status()
        .expect("binding snapshot status")
        .json::<serde_json::Value>()
        .await
        .expect("binding snapshot json");

    let card = &snap["snapshot"]["session_cards"][0];
    assert_eq!(
        card.get("binding_profile_name").and_then(|v| v.as_str()),
        Some("daily")
    );
    assert_eq!(
        card.get("binding_continuity_mode").and_then(|v| v.as_str()),
        Some("default_profile")
    );
    assert_eq!(
        card["effective_model"]
            .get("source")
            .and_then(|v| v.as_str()),
        Some("profile_default")
    );
    assert_eq!(
        card["effective_config_name"]
            .get("value")
            .and_then(|v| v.as_str()),
        Some("test")
    );
    assert_eq!(
        card["effective_config_name"]
            .get("source")
            .and_then(|v| v.as_str()),
        Some("profile_default")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_default_profile_override_applies_to_new_session() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|body: Bytes| async move {
            let json: serde_json::Value =
                serde_json::from_slice(&body).expect("echo upstream json");
            (StatusCode::OK, Json(json))
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", upstream_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut t = HashMap::new();
                t.insert("provider_id".to_string(), "u-bind-runtime".to_string());
                t
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.codex.default_profile = Some("daily".to_string());
    cfg.codex.profiles.insert(
        "daily".to_string(),
        ServiceControlProfile {
            station: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
        },
    );
    cfg.codex.profiles.insert(
        "fast".to_string(),
        ServiceControlProfile {
            station: Some("test".to_string()),
            model: Some("gpt-5.4-fast".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
        },
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let set_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "fast" }))
        .send()
        .await
        .expect("set runtime default profile send");
    assert_eq!(set_default.status(), StatusCode::NO_CONTENT);

    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("session_id", "sid-bind-runtime")
        .body(r#"{"input":"hi"}"#)
        .send()
        .await
        .expect("send runtime binding request")
        .error_for_status()
        .expect("runtime binding request status")
        .json::<serde_json::Value>()
        .await
        .expect("runtime binding request json");

    assert_eq!(
        resp.get("model").and_then(|v| v.as_str()),
        Some("gpt-5.4-fast")
    );
    assert_eq!(
        resp.get("service_tier").and_then(|v| v.as_str()),
        Some("priority")
    );
    assert_eq!(
        resp.get("reasoning")
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str()),
        Some("low")
    );

    let snap = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot?recent_limit=10&stats_days=7",
            proxy_addr
        ))
        .send()
        .await
        .expect("runtime binding snapshot send")
        .error_for_status()
        .expect("runtime binding snapshot status")
        .json::<serde_json::Value>()
        .await
        .expect("runtime binding snapshot json");

    assert_eq!(
        snap.get("default_profile").and_then(|v| v.as_str()),
        Some("fast")
    );
    let card = &snap["snapshot"]["session_cards"][0];
    assert_eq!(
        card.get("binding_profile_name").and_then(|v| v.as_str()),
        Some("fast")
    );
    assert_eq!(
        card["effective_model"]
            .get("value")
            .and_then(|v| v.as_str()),
        Some("gpt-5.4-fast")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_config_meta_override_controls_routing() {
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            (
                StatusCode::OK,
                Json(serde_json::json!({ "upstream": "primary" })),
            )
        }),
    );
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            (
                StatusCode::OK,
                Json(serde_json::json!({ "upstream": "backup" })),
            )
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let mut mgr = ServiceConfigManager {
        active: None,
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: Some("primary".to_string()),
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", primary_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "primary".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: Some("backup".to_string()),
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", backup_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "backup".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let send_request = || async {
        client
            .post(format!("http://{}/v1/responses", proxy_addr))
            .header("content-type", "application/json")
            .body(r#"{"input":"hi"}"#)
            .send()
            .await
            .expect("send request")
            .error_for_status()
            .expect("request status")
            .json::<serde_json::Value>()
            .await
            .expect("request json")
    };

    let resp = send_request().await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("primary")
    );

    let set_disable = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/configs/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "config_name": "primary",
            "enabled": false,
        }))
        .send()
        .await
        .expect("disable primary send");
    assert_eq!(set_disable.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/configs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs send")
        .error_for_status()
        .expect("get configs status")
        .json::<Vec<crate::dashboard_core::ConfigOption>>()
        .await
        .expect("get configs json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config");
    assert!(!primary_cfg.enabled);
    assert!(primary_cfg.configured_enabled);
    assert_eq!(primary_cfg.runtime_enabled_override, Some(false));

    let resp = send_request().await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    let clear_disable = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/configs/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "config_name": "primary",
            "clear_enabled": true,
        }))
        .send()
        .await
        .expect("clear primary disable send");
    assert_eq!(clear_disable.status(), StatusCode::NO_CONTENT);

    let set_primary_level = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/configs/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "config_name": "primary",
            "level": 10,
        }))
        .send()
        .await
        .expect("set primary level send");
    assert_eq!(set_primary_level.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/configs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs after level send")
        .error_for_status()
        .expect("get configs after level status")
        .json::<Vec<crate::dashboard_core::ConfigOption>>()
        .await
        .expect("get configs after level json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config");
    assert_eq!(primary_cfg.level, 10);
    assert_eq!(primary_cfg.configured_level, 1);
    assert_eq!(primary_cfg.runtime_level_override, Some(10));

    let resp = send_request().await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_config_state_override_controls_routing() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let primary_hits_for_route = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_hits = primary_hits_for_route.clone();
            async move {
                primary_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "upstream": "primary" })),
                )
            }
        }),
    );
    let backup_hits_for_route = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_hits = backup_hits_for_route.clone();
            async move {
                backup_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "upstream": "backup" })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let mut mgr = ServiceConfigManager {
        active: None,
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: Some("primary".to_string()),
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", primary_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "primary".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: Some("backup".to_string()),
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", backup_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "backup".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let resp = send_responses_json(&client, proxy_addr, None).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("primary")
    );

    let set_draining = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/configs/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "config_name": "primary",
            "runtime_state": "draining",
        }))
        .send()
        .await
        .expect("set primary draining send");
    assert_eq!(set_draining.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/configs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs after draining send")
        .error_for_status()
        .expect("get configs after draining status")
        .json::<Vec<crate::dashboard_core::ConfigOption>>()
        .await
        .expect("get configs after draining json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config after draining");
    assert_eq!(primary_cfg.runtime_state, RuntimeConfigState::Draining);
    assert_eq!(
        primary_cfg.runtime_state_override,
        Some(RuntimeConfigState::Draining)
    );

    let resp = send_responses_json(&client, proxy_addr, None).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    let set_session_cfg = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/config",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "session_id": "sid-runtime-state",
            "config_name": "primary",
        }))
        .send()
        .await
        .expect("set session config override send");
    assert_eq!(set_session_cfg.status(), StatusCode::NO_CONTENT);

    let resp = send_responses_json(&client, proxy_addr, Some("sid-runtime-state")).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("primary")
    );

    let set_breaker_open = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/configs/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "config_name": "primary",
            "runtime_state": "breaker_open",
        }))
        .send()
        .await
        .expect("set primary breaker open send");
    assert_eq!(set_breaker_open.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/configs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs after breaker open send")
        .error_for_status()
        .expect("get configs after breaker open status")
        .json::<Vec<crate::dashboard_core::ConfigOption>>()
        .await
        .expect("get configs after breaker open json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config after breaker open");
    assert_eq!(primary_cfg.runtime_state, RuntimeConfigState::BreakerOpen);
    assert_eq!(
        primary_cfg.runtime_state_override,
        Some(RuntimeConfigState::BreakerOpen)
    );

    let resp = send_responses_json(&client, proxy_addr, None).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    let primary_before_blocked = primary_hits.load(Ordering::SeqCst);
    let backup_before_blocked = backup_hits.load(Ordering::SeqCst);
    let blocked = send_responses_request(&client, proxy_addr, Some("sid-runtime-state")).await;
    assert_eq!(blocked.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(primary_hits.load(Ordering::SeqCst), primary_before_blocked);
    assert_eq!(backup_hits.load(Ordering::SeqCst), backup_before_blocked);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_stations_alias_works() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::from([("provider_id".to_string(), "u1".to_string())]),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let stations = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get stations send")
        .error_for_status()
        .expect("get stations status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get stations json");
    let primary = stations
        .iter()
        .find(|station| station.name == "test")
        .expect("test station");
    assert!(primary.enabled);
    assert_eq!(primary.runtime_enabled_override, None);

    let set_disable = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "test",
            "enabled": false,
        }))
        .send()
        .await
        .expect("disable station send");
    assert_eq!(set_disable.status(), StatusCode::NO_CONTENT);

    let stations = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get stations after disable send")
        .error_for_status()
        .expect("get stations after disable status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get stations after disable json");
    let primary = stations
        .iter()
        .find(|station| station.name == "test")
        .expect("test station after disable");
    assert!(!primary.enabled);
    assert_eq!(primary.runtime_enabled_override, Some(false));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_auth_file_cache_refreshes_after_source_change() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    let base = make_temp_test_dir();
    let codex_home = base.join("codex-home");
    let claude_home = base.join("claude-home");
    let codex_auth = codex_home.join("auth.json");
    let claude_settings = claude_home.join("settings.json");

    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
        scoped.set_path("CLAUDE_HOME", &claude_home);
    }

    write_text_file(&codex_auth, r#"{"OPENAI_API_KEY":"sk-first"}"#);
    write_text_file(
        &claude_settings,
        r#"{"env":{"ANTHROPIC_API_KEY":"claude-first"}}"#,
    );

    assert_eq!(
        super::codex_auth_json_value("OPENAI_API_KEY"),
        Some("sk-first".to_string())
    );
    assert_eq!(
        super::claude_settings_env_value("ANTHROPIC_API_KEY"),
        Some("claude-first".to_string())
    );

    sleep(Duration::from_millis(30)).await;
    write_text_file(&codex_auth, r#"{"OPENAI_API_KEY":"sk-second"}"#);
    write_text_file(
        &claude_settings,
        r#"{"env":{"ANTHROPIC_API_KEY":"claude-second"}}"#,
    );
    sleep(Duration::from_millis(30)).await;

    assert_eq!(
        super::codex_auth_json_value("OPENAI_API_KEY"),
        Some("sk-second".to_string())
    );
    assert_eq!(
        super::claude_settings_env_value("ANTHROPIC_API_KEY"),
        Some("claude-second".to_string())
    );
}

#[tokio::test]
async fn proxy_admin_routes_require_loopback_or_token_for_remote_access() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set(super::ADMIN_TOKEN_ENV_VAR, "remote-secret");
    }

    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let remote_addr = std::net::SocketAddr::from(([203, 0, 113, 7], 43123));

    let mut denied_req = Request::builder()
        .uri("/__codex_helper/api/v1/capabilities")
        .body(Body::empty())
        .expect("build denied request");
    denied_req.extensions_mut().insert(ConnectInfo(remote_addr));
    let denied = app
        .clone()
        .oneshot(denied_req)
        .await
        .expect("denied response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied_body = to_bytes(denied.into_body(), usize::MAX)
        .await
        .expect("denied body");
    let denied_text = String::from_utf8_lossy(&denied_body);
    assert!(denied_text.contains(super::ADMIN_TOKEN_HEADER));

    let mut allowed_req = Request::builder()
        .uri("/__codex_helper/api/v1/capabilities")
        .header(super::ADMIN_TOKEN_HEADER, "remote-secret")
        .body(Body::empty())
        .expect("build allowed request");
    allowed_req
        .extensions_mut()
        .insert(ConnectInfo(remote_addr));
    let allowed = app.oneshot(allowed_req).await.expect("allowed response");
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn proxy_split_listeners_isolate_admin_routes_from_proxy_traffic() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let proxy_app = crate::proxy::proxy_only_router(proxy.clone());
    let admin_app = crate::proxy::admin_listener_router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(proxy_app);
    let (admin_addr, admin_handle) = spawn_axum_server(admin_app);

    let client = reqwest::Client::new();

    let proxy_admin = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/capabilities",
            proxy_addr
        ))
        .send()
        .await
        .expect("proxy admin send");
    assert_eq!(proxy_admin.status(), StatusCode::NOT_FOUND);

    let admin_caps = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/capabilities",
            admin_addr
        ))
        .send()
        .await
        .expect("admin caps send")
        .error_for_status()
        .expect("admin caps status")
        .json::<serde_json::Value>()
        .await
        .expect("admin caps json");
    assert_eq!(
        admin_caps.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );

    let admin_proxy = client
        .post(format!("http://{}/v1/responses", admin_addr))
        .body("{}")
        .send()
        .await
        .expect("admin proxy send");
    assert_eq!(admin_proxy.status(), StatusCode::NOT_FOUND);

    proxy_handle.abort();
    admin_handle.abort();
}

#[tokio::test]
async fn proxy_only_router_exposes_admin_discovery_document() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::proxy_only_router_with_admin_base_url(
        proxy,
        Some("http://127.0.0.1:4100".to_string()),
    );

    let discovery = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/codex-helper-admin")
                .body(Body::empty())
                .expect("build discovery request"),
        )
        .await
        .expect("discovery response");

    assert_eq!(discovery.status(), StatusCode::OK);
    let body = to_bytes(discovery.into_body(), usize::MAX)
        .await
        .expect("discovery body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("discovery json");
    assert_eq!(json.get("api_version").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        json.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );
    assert_eq!(
        json.get("admin_base_url").and_then(|v| v.as_str()),
        Some("http://127.0.0.1:4100")
    );
}

#[tokio::test]
async fn proxy_failover_retries_502_then_uses_second_upstream() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "nope" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "upstream": 2 })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body}"
    );
    // Two-layer model: retry current upstream first, then fail over.
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_same_upstream_retries_502_then_succeeds_without_failover() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            let n = u1_hits.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "err": "first attempt 502" })),
                )
            } else {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "ok": true, "upstream": 1 })),
                )
            }
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "upstream": 2 })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::SameUpstream),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains(r#""upstream":1"#),
        "expected response from upstream1, got: {body}"
    );
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_502_when_no_internal_retry() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "always 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    // First request hits upstream1, gets a retryable 502, and fails over to upstream2.
    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );

    // Second request should now go directly to upstream2 thanks to the cooldown on upstream1.
    let (status2, body2) = {
        let mut last_status = StatusCode::INTERNAL_SERVER_ERROR;
        let mut last_body: Bytes = Bytes::new();
        for attempt in 0..3 {
            let resp2 = client
                .post(format!("http://{}/v1/responses", proxy_addr))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .body(r#"{"model":"gpt","input":"hi"}"#)
                .send()
                .await
                .expect("send");
            last_status = resp2.status();
            last_body = resp2.bytes().await.expect("read bytes");
            if last_status == StatusCode::OK {
                break;
            }
            if attempt < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        }
        (last_status, last_body)
    };
    assert_eq!(status2, StatusCode::OK);
    let body_s = String::from_utf8_lossy(&body2);
    assert!(
        body_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body_s}"
    );

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_transport_error_when_no_internal_retry() {
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let unused = reserve_unused_local_addr();

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(vec!["upstream_transport_error".to_string()]),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", unused),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body = resp2.bytes().await.expect("read bytes");
    let body_s = String::from_utf8_lossy(&body);
    assert!(
        body_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body_s}"
    );
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_cloudflare_challenge_when_no_internal_retry() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            let mut resp = Response::new(Body::from(
                "<html><body>/cdn-cgi/ challenge-platform __CF$cv$params</body></html>",
            ));
            *resp.status_mut() = StatusCode::FORBIDDEN;
            resp.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            resp.headers_mut()
                .insert("server", HeaderValue::from_static("cloudflare"));
            resp.headers_mut()
                .insert("cf-ray", HeaderValue::from_static("test"));
            resp
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(vec!["cloudflare_challenge".to_string()]),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(60),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.bytes().await.expect("read bytes");
    let body2_s = String::from_utf8_lossy(&body2);
    assert!(
        body2_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body2_s}"
    );

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_multi_config_failover_across_requests_respects_cooldown() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let p_hits = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            p_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "primary 502" })),
            )
        }),
    );
    let (p_addr, p_handle) = spawn_axum_server(primary);

    let b_hits = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            b_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":\"backup\"}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(backup);

    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };

    let mut mgr = ServiceConfigManager {
        active: Some("primary".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", p_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "primary".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: None,
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", b_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "backup".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry,
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":"backup""#),
        "expected response from backup, got: {body1_s}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.bytes().await.expect("read bytes");
    let body2_s = String::from_utf8_lossy(&body2);
    assert!(
        body2_s.contains(r#""upstream":"backup""#),
        "expected response from backup, got: {body2_s}"
    );

    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    p_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_failover_when_502_is_not_retryable_and_threshold_not_reached() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "always 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::BAD_GATEWAY);

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::BAD_GATEWAY);

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_retries_each_upstream_once_and_stops_when_all_avoided() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "u1 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "u2 502" })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_streaming_parses_usage_even_when_usage_is_late_in_stream() {
    // Large prefix with no `data:` lines: should push the stream well past 1MB without triggering JSON parse.
    // The final `data:` line includes `response.usage`, which codex-helper should still detect.
    let prefix = Bytes::from(format!("event: {}\n\n", "x".repeat(4096)));
    let n = 260usize; // ~1.1MB before usage
    let usage = Bytes::from(
        "event: response.completed\n\
data: {\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
    );

    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let prefix = prefix.clone();
            let usage = usage.clone();
            async move {
                // Use a non-streaming body here to avoid flaky chunked-decoding failures on some
                // hyper/reqwest versions, while still exercising the proxy SSE path and the
                // "usage appears after >1MB of non-data bytes" scenario.
                let mut body = Vec::with_capacity(prefix.len().saturating_mul(n) + usage.len());
                for _ in 0..n {
                    body.extend_from_slice(prefix.as_ref());
                }
                body.extend_from_slice(usage.as_ref());
                let mut resp = Response::new(Body::from(body));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", u_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let mut drained_ok = false;
    let mut last_status: Option<StatusCode> = None;
    for _ in 0..3 {
        let resp = client
            .post(format!("http://{}/v1/responses", proxy_addr))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(r#"{"model":"gpt","input":"hi"}"#)
            .send()
            .await
            .expect("send");
        last_status = Some(resp.status());
        if resp.status() == StatusCode::OK && resp.bytes().await.is_ok() {
            drained_ok = true;
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(last_status, Some(StatusCode::OK));
    assert!(
        drained_ok,
        "expected to drain SSE body without decode error"
    );

    let mut finished = Vec::new();
    for _ in 0..100 {
        finished = state.list_recent_finished(10).await;
        if finished.iter().any(|f| f.usage.is_some()) {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !finished.is_empty(),
        "expected finished request to be recorded"
    );
    let u = finished
        .iter()
        .find_map(|f| f.usage.as_ref())
        .expect("usage should be parsed");
    assert_eq!(u.total_tokens, 3);

    proxy_handle.abort();
    u_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_retry_or_failover_on_400() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "err": "bad request" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_retries_404_when_enabled() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            StatusCode::NOT_FOUND
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("400-599".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    // Two-layer model: retry current upstream first, then fail over.
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    let u2 = upstream2_hits.load(Ordering::SeqCst);
    assert!(
        matches!(u2, 1 | 2),
        "expected upstream2 hits to be 1..=2 (transport flake tolerance), got {u2}"
    );

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_failover_on_non_retryable_client_error_class() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "type": "invalid_request_error",
                        "message": "`tool_use` ids must be unique"
                    }
                })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("400-599".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_skips_upstreams_that_do_not_support_model() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "err": "should not hit" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "upstream": 2 })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: {
                    let mut m = HashMap::new();
                    m.insert("other-*".to_string(), true);
                    m
                },
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: {
                    let mut m = HashMap::new();
                    m.insert("gpt-*".to_string(), true);
                    m
                },
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-4","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 0);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_applies_model_mapping_to_request_body() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));

    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |body: axum::body::Bytes| async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let v: serde_json::Value =
                serde_json::from_slice(&body).expect("json body should parse");
            let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("");
            if model == "anthropic/claude-sonnet-4" {
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "model": model })),
                )
            }
        }),
    );
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", u_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: {
                let mut m = HashMap::new();
                m.insert("anthropic/claude-*".to_string(), true);
                m
            },
            model_mapping: {
                let mut m = HashMap::new();
                m.insert("claude-*".to_string(), "anthropic/claude-*".to_string());
                m
            },
        }],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-sonnet-4","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u_handle.abort();
}

#[tokio::test]
async fn proxy_falls_back_to_level_2_config_after_retryable_failure() {
    let level1_hits = Arc::new(AtomicUsize::new(0));
    let level2_hits = Arc::new(AtomicUsize::new(0));

    let l1_hits = level1_hits.clone();
    let level1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            l1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "level1 nope" })),
            )
        }),
    );
    let (l1_addr, l1_handle) = spawn_axum_server(level1);

    let l2_hits = level2_hits.clone();
    let level2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            l2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (l2_addr, l2_handle) = spawn_axum_server(level2);

    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };

    let mut mgr = ServiceConfigManager {
        active: Some("level-1".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "level-1".to_string(),
        ServiceConfig {
            name: "level-1".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", l1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "level-2".to_string(),
        ServiceConfig {
            name: "level-2".to_string(),
            alias: None,
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", l2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry,
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    // Two-layer model: retry current config/upstream first, then fail over to next config.
    assert_eq!(level1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(level2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    l1_handle.abort();
    l2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_can_switch_configs_with_same_level() {
    let c1_hits = Arc::new(AtomicUsize::new(0));
    let c2_hits = Arc::new(AtomicUsize::new(0));

    let c1_hits2 = c1_hits.clone();
    let config1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c1_hits2.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "config1 nope" })),
            )
        }),
    );
    let (c1_addr, c1_handle) = spawn_axum_server(config1);

    let c2_hits2 = c2_hits.clone();
    let config2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c2_hits2.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (c2_addr, c2_handle) = spawn_axum_server(config2);

    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };

    let mut mgr = ServiceConfigManager {
        active: Some("config-1".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "config-1".to_string(),
        ServiceConfig {
            name: "config-1".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", c1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "config-2".to_string(),
        ServiceConfig {
            name: "config-2".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", c2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry,
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    // Two-layer model: retry current config/upstream first, then fail over to next config.
    assert_eq!(c1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(c2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    c1_handle.abort();
    c2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_can_switch_configs_with_same_level_on_404() {
    let c1_hits = Arc::new(AtomicUsize::new(0));
    let c2_hits = Arc::new(AtomicUsize::new(0));

    let c1_hits2 = c1_hits.clone();
    let config1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c1_hits2.fetch_add(1, Ordering::SeqCst);
            StatusCode::NOT_FOUND
        }),
    );
    let (c1_addr, c1_handle) = spawn_axum_server(config1);

    let c2_hits2 = c2_hits.clone();
    let config2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c2_hits2.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (c2_addr, c2_handle) = spawn_axum_server(config2);

    let cfg = ProxyConfig {
        version: Some(1),
        codex: {
            let mut mgr = ServiceConfigManager {
                active: Some("config1".to_string()),
                ..Default::default()
            };
            mgr.configs.insert(
                "config1".to_string(),
                ServiceConfig {
                    name: "config1".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: format!("http://{}/v1", c1_addr),
                        auth: UpstreamAuth {
                            auth_token: None,
                            auth_token_env: None,
                            api_key: None,
                            api_key_env: None,
                        },
                        tags: HashMap::new(),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    }],
                },
            );
            mgr.configs.insert(
                "config2".to_string(),
                ServiceConfig {
                    name: "config2".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: format!("http://{}/v1", c2_addr),
                        auth: UpstreamAuth {
                            auth_token: None,
                            auth_token_env: None,
                            api_key: None,
                            api_key_env: None,
                        },
                        tags: HashMap::new(),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    }],
                },
            );
            mgr
        },
        claude: ServiceConfigManager::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    // 404 is treated as provider/config-level failure by default (no upstream retries).
    assert_eq!(c1_hits.load(Ordering::SeqCst), 1);
    let c2 = c2_hits.load(Ordering::SeqCst);
    assert!(
        matches!(c2, 1 | 2),
        "expected config2 hits to be 1..=2 (transport flake tolerance), got {c2}"
    );

    proxy_handle.abort();
    c1_handle.abort();
    c2_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_config_reports_resolved_retry_profile() {
    let proxy_client = Client::new();
    let retry = RetryConfig {
        profile: Some(RetryProfileName::CostPrimary),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:1/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let v: serde_json::Value = client
        .get(format!(
            "http://{}/__codex_helper/config/runtime",
            proxy_addr
        ))
        .send()
        .await
        .expect("send")
        .error_for_status()
        .expect("status ok")
        .json()
        .await
        .expect("json");

    let retry = v.get("retry").expect("retry field");
    assert!(
        retry.get("profile").is_none(),
        "runtime endpoint should expose resolved retry config (no profile field)"
    );
    assert!(retry.get("strategy").is_none());
    assert!(retry.get("max_attempts").is_none());
    assert_eq!(
        retry
            .get("upstream")
            .and_then(|x| x.get("strategy"))
            .and_then(|x| x.as_str()),
        Some("same_upstream")
    );
    assert_eq!(
        retry
            .get("provider")
            .and_then(|x| x.get("strategy"))
            .and_then(|x| x.as_str()),
        Some("failover")
    );
    assert_eq!(
        retry
            .get("provider")
            .and_then(|x| x.get("max_attempts"))
            .and_then(|x| x.as_u64()),
        Some(2)
    );
    assert_eq!(
        retry
            .get("cooldown_backoff_factor")
            .and_then(|x| x.as_u64()),
        Some(2)
    );
    assert_eq!(
        retry
            .get("cooldown_backoff_max_secs")
            .and_then(|x| x.as_u64()),
        Some(900)
    );

    proxy_handle.abort();
}
