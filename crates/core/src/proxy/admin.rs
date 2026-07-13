use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;

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
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string().trim_end_matches('/').to_string())
}

pub fn admin_loopback_addr_for_proxy_port(proxy_port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], admin_port_for_proxy_port(proxy_port)))
}

fn is_admin_path(path: &str) -> bool {
    path == "/__codex_helper" || path.starts_with("/__codex_helper/")
}

fn is_proxy_reserved_path(path: &str) -> bool {
    is_admin_path(path) || path == "/.well-known/codex-helper-admin"
}

pub(super) async fn require_admin_access(
    State(access): State<AdminAccessConfig>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Result<Response<Body>, (StatusCode, String)> {
    let provided = req
        .headers()
        .get(ADMIN_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(expected) = access.token.as_deref() {
        if Some(expected) == provided {
            return Ok(next.run(req).await);
        }
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "admin routes require {ADMIN_TOKEN_HEADER} when {ADMIN_TOKEN_ENV_VAR} is configured"
            ),
        ));
    }

    if peer_addr.ip().is_loopback() {
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
    if is_proxy_reserved_path(req.uri().path()) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_admin_base_url_discards_proxy_request_path() {
        assert_eq!(
            admin_base_url_from_proxy_base_url("http://127.0.0.1:3211/v1"),
            Some("http://127.0.0.1:4211".to_string())
        );
        assert_eq!(
            admin_base_url_from_proxy_base_url("http://127.0.0.1:3211/backend-api/codex/responses"),
            Some("http://127.0.0.1:4211".to_string())
        );
    }
}
