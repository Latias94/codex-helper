use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use codex_helper_core::control_plane_client::{
    ControlPlaneClient, ControlPlaneEndpoint, configured_local_admin_token_env,
    is_loopback_control_plane_base_url,
};
use codex_helper_core::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use codex_helper_core::proxy::{
    ADMIN_PORT_OFFSET, admin_port_for_proxy_port, local_admin_base_url_for_proxy_port,
    local_proxy_base_url,
};
use codex_helper_core::request_chain::{RequestChainExport, RequestChainSelector};
use serde::{Deserialize, Serialize};

use crate::error::CommandError;

pub(crate) const DEFAULT_PROXY_PORT: u16 = 3211;
const ADMIN_BASE_ENV: &str = "CODEX_HELPER_DESKTOP_ADMIN_URL";
const OPERATOR_SERVICE_NAME: &str = "codex";

static LAST_READY_BY_ENDPOINT: OnceLock<Mutex<HashMap<String, OperatorReadModel>>> =
    OnceLock::new();

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminEndpointConfig {
    pub proxy_port: u16,
    pub admin_port: u16,
    pub proxy_base_url: String,
    pub admin_base_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminReadModel {
    pub endpoint: AdminEndpointConfig,
    pub operator_read_model: OperatorReadModel,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestChainPayload {
    pub trace_id: Option<String>,
    pub request_id: Option<u64>,
    pub session: Option<String>,
    pub limit: Option<usize>,
}

#[tauri::command]
pub async fn get_admin_read_model() -> Result<AdminReadModel, CommandError> {
    let endpoint = admin_endpoint_config()?;
    let client = control_plane_client(&endpoint)?;
    Ok(refresh_admin_read_model(endpoint, &client).await)
}

#[tauri::command]
pub async fn get_request_chain(
    payload: RequestChainPayload,
) -> Result<RequestChainExport, CommandError> {
    if !request_chain_payload_has_selector(&payload) {
        return Err(CommandError::new(
            "desktop_request_chain_selector_required",
            "traceId, requestId, or session is required",
            false,
        ));
    }
    let endpoint = admin_endpoint_config()?;
    let client = control_plane_client(&endpoint)?;
    client
        .request_chain(
            RequestChainSelector {
                trace_id: payload.trace_id,
                request_id: payload.request_id,
                session_id: payload.session,
            },
            payload.limit.unwrap_or(20),
        )
        .await
        .map_err(control_plane_request_error)
}

pub(crate) fn control_plane_client(
    endpoint: &AdminEndpointConfig,
) -> Result<ControlPlaneClient, CommandError> {
    let token_env = is_loopback_control_plane_base_url(&endpoint.admin_base_url)
        .then(configured_local_admin_token_env)
        .flatten();
    let endpoint =
        ControlPlaneEndpoint::new(endpoint.admin_base_url.clone(), token_env).map_err(|error| {
            CommandError::new(
                "desktop_admin_invalid_url",
                format!("invalid desktop admin endpoint: {error}"),
                false,
            )
        })?;
    ControlPlaneClient::new(endpoint).map_err(|error| {
        CommandError::new(
            "desktop_admin_client_error",
            format!("failed to build admin API client: {error}"),
            false,
        )
    })
}

async fn refresh_admin_read_model(
    endpoint: AdminEndpointConfig,
    client: &ControlPlaneClient,
) -> AdminReadModel {
    let cache_key = endpoint.admin_base_url.clone();
    let previous = last_ready_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&cache_key)
        .cloned();
    let operator_read_model = client
        .refresh_operator_read_model(OPERATOR_SERVICE_NAME, previous.as_ref())
        .await;

    let mut cache = last_ready_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match operator_read_model.status {
        OperatorReadStatus::Ready => {
            cache.insert(cache_key, operator_read_model.clone());
        }
        OperatorReadStatus::Stale => {}
        OperatorReadStatus::Disconnected | OperatorReadStatus::AuthRequired => {
            cache.remove(&cache_key);
        }
    }

    AdminReadModel {
        endpoint,
        operator_read_model,
    }
}

fn last_ready_cache() -> &'static Mutex<HashMap<String, OperatorReadModel>> {
    LAST_READY_BY_ENDPOINT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn control_plane_request_error(error: impl std::fmt::Display) -> CommandError {
    let message = error.to_string();
    let retryable = !message.contains("not trusted") && !message.contains("requires");
    CommandError::new("desktop_admin_request_failed", message, retryable)
}

pub(crate) fn admin_endpoint_config() -> Result<AdminEndpointConfig, CommandError> {
    match std::env::var(ADMIN_BASE_ENV) {
        Ok(base) => admin_endpoint_config_from_override(Some(base.trim())),
        Err(std::env::VarError::NotPresent) => admin_endpoint_config_from_override(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(CommandError::new(
            "desktop_admin_invalid_url",
            format!("{ADMIN_BASE_ENV} must contain a valid Unicode URL"),
            false,
        )),
    }
}

fn admin_endpoint_config_from_override(
    admin_base_url: Option<&str>,
) -> Result<AdminEndpointConfig, CommandError> {
    match admin_base_url {
        Some(value) => config_from_admin_base_url(value),
        None => Ok(default_admin_endpoint_config()),
    }
}

fn default_admin_endpoint_config() -> AdminEndpointConfig {
    AdminEndpointConfig {
        proxy_port: DEFAULT_PROXY_PORT,
        admin_port: admin_port_for_proxy_port(DEFAULT_PROXY_PORT),
        proxy_base_url: local_proxy_base_url(DEFAULT_PROXY_PORT),
        admin_base_url: local_admin_base_url_for_proxy_port(DEFAULT_PROXY_PORT),
    }
}

fn config_from_admin_base_url(value: &str) -> Result<AdminEndpointConfig, CommandError> {
    let admin_base_url = codex_helper_core::control_plane_client::normalize_control_plane_base_url(
        value,
    )
    .map_err(|error| {
        CommandError::new(
            "desktop_admin_invalid_url",
            format!("invalid {ADMIN_BASE_ENV}: {error}"),
            false,
        )
    })?;
    let url = reqwest::Url::parse(&admin_base_url).map_err(|error| {
        CommandError::new(
            "desktop_admin_invalid_url",
            format!("invalid {ADMIN_BASE_ENV}: {error}"),
            false,
        )
    })?;
    let admin_port = url.port_or_known_default().ok_or_else(|| {
        CommandError::new(
            "desktop_admin_invalid_url",
            format!("invalid {ADMIN_BASE_ENV}: URL has no effective port"),
            false,
        )
    })?;
    let proxy_port = if admin_port > ADMIN_PORT_OFFSET {
        admin_port.saturating_sub(ADMIN_PORT_OFFSET)
    } else {
        DEFAULT_PROXY_PORT
    };
    Ok(AdminEndpointConfig {
        proxy_port,
        admin_port,
        proxy_base_url: local_proxy_base_url(proxy_port),
        admin_base_url,
    })
}

fn request_chain_payload_has_selector(payload: &RequestChainPayload) -> bool {
    payload
        .trace_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || payload.request_id.is_some()
        || payload
            .session
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use codex_helper_core::dashboard_core::{
        ApiV1OperatorSummary, OperatorReadData, OperatorReadIssue, OperatorRevisionBundle,
    };

    use super::*;

    fn ready_operator_model(revision: u64) -> OperatorReadModel {
        OperatorReadModel::ready(
            OPERATOR_SERVICE_NAME,
            1_700_000_000_000 + revision,
            OperatorRevisionBundle {
                runtime_revision: revision,
                runtime_digest: format!("runtime-{revision}"),
                route_digest: format!("route-{revision}"),
                catalog_revision: format!("catalog-{revision}"),
                pricing_revision: format!("pricing-{revision}"),
                operator_pricing_revision: format!("operator-pricing-{revision}"),
                policy_revision: revision + 1,
                ledger_revision: format!("operator-ledger-v1:test-store:{}", revision + 2),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name: OPERATOR_SERVICE_NAME.to_string(),
                    runtime: Default::default(),
                    counts: Default::default(),
                    retry: Default::default(),
                    sessions: Vec::new(),
                    profiles: Vec::new(),
                    providers: Vec::new(),
                },
                active_requests: Vec::new(),
                recent_requests: Vec::new(),
                usage_summaries: Vec::new(),
                usage_day: Default::default(),
                usage_rollup: Default::default(),
                stats_5m: Default::default(),
                stats_1h: Default::default(),
                pricing_catalog: Default::default(),
                provider_balances: Vec::new(),
            },
        )
    }

    fn spawn_operator_response_sequence(
        responses: Vec<(String, String)>,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test control plane");
        let address = listener.local_addr().expect("test listener address");
        let server = std::thread::spawn(move || {
            responses
                .into_iter()
                .map(|(status, body)| {
                    let (mut stream, _) = listener.accept().expect("accept operator request");
                    let mut request = [0_u8; 2048];
                    let read = stream.read(&mut request).expect("read operator request");
                    let request_line = String::from_utf8_lossy(&request[..read])
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream
                        .write_all(response.as_bytes())
                        .expect("write operator response");
                    request_line
                })
                .collect()
        });
        (address, server)
    }

    async fn assert_last_ready_cache_auth_reset_sequence(auth_status: &str, revision: u64) {
        let expected_ready = ready_operator_model(revision);
        let ready_body = serde_json::to_string(&expected_ready).expect("serialize ready model");
        let (address, server) = spawn_operator_response_sequence(vec![
            ("200 OK".to_string(), ready_body),
            ("503 Service Unavailable".to_string(), String::new()),
            (auth_status.to_string(), String::new()),
            ("503 Service Unavailable".to_string(), String::new()),
        ]);
        let base_url = format!("http://{address}");
        let client = control_plane_client(
            &config_from_admin_base_url(&base_url).expect("build loopback test endpoint"),
        )
        .expect("build shared control-plane client");

        let ready = refresh_admin_read_model(
            config_from_admin_base_url(&base_url).expect("build ready endpoint"),
            &client,
        )
        .await
        .operator_read_model;
        assert_eq!(ready, expected_ready);

        let stale = refresh_admin_read_model(
            config_from_admin_base_url(&base_url).expect("build stale endpoint"),
            &client,
        )
        .await
        .operator_read_model;
        assert_eq!(stale.status, OperatorReadStatus::Stale);
        assert_eq!(stale.issue, Some(OperatorReadIssue::RefreshFailed));
        assert_eq!(stale.captured_at_ms, expected_ready.captured_at_ms);
        assert_eq!(stale.revisions, expected_ready.revisions);
        assert_eq!(stale.data, expected_ready.data);

        let auth_required = refresh_admin_read_model(
            config_from_admin_base_url(&base_url).expect("build auth endpoint"),
            &client,
        )
        .await
        .operator_read_model;
        assert_eq!(auth_required.status, OperatorReadStatus::AuthRequired);
        assert_eq!(auth_required.issue, Some(OperatorReadIssue::AuthRequired));
        assert!(auth_required.revisions.is_none());
        assert!(auth_required.data.is_none());

        let disconnected = refresh_admin_read_model(
            config_from_admin_base_url(&base_url).expect("build disconnected endpoint"),
            &client,
        )
        .await
        .operator_read_model;
        assert_eq!(disconnected.status, OperatorReadStatus::Disconnected);
        assert_eq!(disconnected.issue, Some(OperatorReadIssue::Disconnected));
        assert!(disconnected.revisions.is_none());
        assert!(disconnected.data.is_none());
        assert!(
            !last_ready_cache()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&base_url)
        );

        let request_lines = server.join().expect("join test control plane");
        assert_eq!(request_lines.len(), 4);
        assert!(request_lines.iter().all(|request_line| {
            request_line == "GET /__codex_helper/api/v1/operator/read-model HTTP/1.1"
        }));
    }

    #[test]
    fn request_chain_payload_accepts_any_supported_selector() {
        let payload = RequestChainPayload {
            trace_id: Some(" trace/with space ".to_string()),
            request_id: Some(42),
            session: Some("session a".to_string()),
            limit: Some(10),
        };

        assert!(request_chain_payload_has_selector(&payload));
    }

    #[test]
    fn request_chain_payload_requires_selector() {
        let payload = RequestChainPayload {
            trace_id: Some(" ".to_string()),
            request_id: None,
            session: None,
            limit: None,
        };

        assert!(!request_chain_payload_has_selector(&payload));
    }

    #[test]
    fn configured_admin_endpoint_fails_closed_for_invalid_override() {
        let error = admin_endpoint_config_from_override(Some("http://nas.example:4211"))
            .expect_err("invalid configured endpoint must not fall back to localhost");

        assert_eq!(error.code, "desktop_admin_invalid_url");
        assert!(
            error.message.contains("HTTPS"),
            "unexpected: {}",
            error.message
        );
    }

    #[test]
    fn desktop_remote_endpoint_does_not_reuse_the_local_admin_token() {
        let endpoint = AdminEndpointConfig {
            proxy_port: DEFAULT_PROXY_PORT,
            admin_port: admin_port_for_proxy_port(DEFAULT_PROXY_PORT),
            proxy_base_url: local_proxy_base_url(DEFAULT_PROXY_PORT),
            admin_base_url: "https://relay.example:4211".to_string(),
        };

        let error = control_plane_client(&endpoint)
            .expect_err("desktop must not send the local token to a remote endpoint");

        assert!(error.message.contains("admin_token_env"));
    }

    #[test]
    fn refresh_uses_one_canonical_operator_read_model_get() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test control plane");
        let address = listener.local_addr().expect("test listener address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept operator request");
            let mut request = [0_u8; 2048];
            let read = stream.read(&mut request).expect("read operator request");
            let request = String::from_utf8_lossy(&request[..read]);
            let request_line = request.lines().next().unwrap_or_default().to_string();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                )
                .expect("write invalid operator fixture");
            request_line
        });
        let endpoint = config_from_admin_base_url(&format!("http://{address}"))
            .expect("build loopback test endpoint");
        let client = control_plane_client(&endpoint).expect("build shared control-plane client");

        let result = tauri::async_runtime::block_on(refresh_admin_read_model(endpoint, &client));
        let request_line = server.join().expect("join test control plane");

        assert_eq!(
            request_line,
            "GET /__codex_helper/api/v1/operator/read-model HTTP/1.1"
        );
        assert_eq!(
            result.operator_read_model.status,
            OperatorReadStatus::Disconnected
        );
        assert!(result.operator_read_model.data.is_none());
        assert!(result.operator_read_model.revisions.is_none());
    }

    #[test]
    fn last_ready_cache_is_cleared_after_auth_failure() {
        tauri::async_runtime::block_on(async {
            assert_last_ready_cache_auth_reset_sequence("401 Unauthorized", 401).await;
            assert_last_ready_cache_auth_reset_sequence("403 Forbidden", 403).await;
        });
    }
}
