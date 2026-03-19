use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

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

use super::{
    ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN_HEADER, CLIENT_NAME_HEADER, claude_settings_env_value,
    codex_auth_json_value,
};
use crate::config::{
    GroupConfigV2, GroupMemberRefV2, ProviderConfigV2, ProviderEndpointV2, ProxyConfig,
    ProxyConfigV2, RetryConfig, RetryProfileName, RetryStrategy, ServiceConfig,
    ServiceConfigManager, ServiceControlProfile, ServiceViewV2, UiConfig, UpstreamAuth,
    UpstreamConfig,
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

async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
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

mod api_admin;
mod failover;
mod routing_profiles;
