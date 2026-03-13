use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;

use crate::dashboard_core::RemoteAdminAccessCapabilities;

use super::{ADMIN_PORT_OFFSET, ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN_HEADER};

#[derive(Clone)]
pub(super) struct AdminAccessConfig {
    token: Option<String>,
}

impl AdminAccessConfig {
    pub(super) fn from_env() -> Self {
        let token = std::env::var(ADMIN_TOKEN_ENV_VAR)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Self { token }
    }
}

pub(super) fn admin_access_capabilities() -> RemoteAdminAccessCapabilities {
    let access = AdminAccessConfig::from_env();
    RemoteAdminAccessCapabilities {
        loopback_without_token: true,
        remote_requires_token: true,
        remote_enabled: access.token.is_some(),
        token_header: ADMIN_TOKEN_HEADER.to_string(),
        token_env_var: ADMIN_TOKEN_ENV_VAR.to_string(),
    }
}

pub fn admin_port_for_proxy_port(proxy_port: u16) -> u16 {
    if proxy_port <= u16::MAX - ADMIN_PORT_OFFSET {
        proxy_port + ADMIN_PORT_OFFSET
    } else if proxy_port > ADMIN_PORT_OFFSET {
        proxy_port - ADMIN_PORT_OFFSET
    } else {
        1
    }
}

pub fn local_proxy_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub fn local_admin_base_url_for_proxy_port(proxy_port: u16) -> String {
    local_proxy_base_url(admin_port_for_proxy_port(proxy_port))
}

pub fn admin_base_url_from_proxy_base_url(proxy_base_url: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(proxy_base_url).ok()?;
    let port = url.port_or_known_default()?;
    url.set_port(Some(admin_port_for_proxy_port(port))).ok()?;
    Some(url.to_string().trim_end_matches('/').to_string())
}

pub fn admin_loopback_addr_for_proxy_port(proxy_port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], admin_port_for_proxy_port(proxy_port)))
}

fn is_admin_path(path: &str) -> bool {
    path == "/__codex_helper" || path.starts_with("/__codex_helper/")
}

#[derive(Clone, serde::Serialize)]
pub(super) struct ProxyAdminDiscovery {
    pub(super) api_version: u32,
    pub(super) service_name: &'static str,
    pub(super) admin_base_url: String,
}

pub(super) async fn require_admin_access(
    State(access): State<AdminAccessConfig>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Result<Response<Body>, (StatusCode, String)> {
    if peer_addr.ip().is_loopback() {
        return Ok(next.run(req).await);
    }

    let provided = req
        .headers()
        .get(ADMIN_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let remote_allowed = access
        .token
        .as_deref()
        .is_some_and(|expected| Some(expected) == provided);
    if remote_allowed {
        return Ok(next.run(req).await);
    }

    Err((
        StatusCode::FORBIDDEN,
        format!(
            "admin routes are loopback-only; set {ADMIN_TOKEN_ENV_VAR} and send {ADMIN_TOKEN_HEADER} to allow remote access"
        ),
    ))
}

pub(super) async fn require_admin_path_only(
    req: Request<Body>,
    next: Next,
) -> Result<Response<Body>, StatusCode> {
    if is_admin_path(req.uri().path()) {
        return Ok(next.run(req).await);
    }
    Err(StatusCode::NOT_FOUND)
}

pub(super) async fn reject_admin_paths_from_proxy(
    req: Request<Body>,
    next: Next,
) -> Result<Response<Body>, StatusCode> {
    if is_admin_path(req.uri().path()) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(next.run(req).await)
}
