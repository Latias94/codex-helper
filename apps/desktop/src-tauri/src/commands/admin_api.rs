use std::time::Duration;

use codex_helper_core::proxy::{
    ADMIN_PORT_OFFSET, ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN_HEADER, RuntimeStatusResponse,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{CommandError, DesktopError};

pub(crate) const DEFAULT_PROXY_PORT: u16 = 3211;
const ADMIN_BASE_ENV: &str = "CODEX_HELPER_DESKTOP_ADMIN_URL";
const REQUEST_TIMEOUT_MS: u64 = 2_500;

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
    pub operator_summary: Value,
    pub runtime_status: Option<RuntimeStatusResponse>,
    pub providers: Vec<Value>,
    pub recent_requests: Vec<Value>,
    pub usage_summary: Vec<Value>,
}

#[tauri::command]
pub async fn get_admin_read_model() -> Result<AdminReadModel, CommandError> {
    let endpoint = admin_endpoint_config();
    let client = admin_client()?;

    let operator_summary: Value = get_json(
        &client,
        &endpoint.admin_base_url,
        "/__codex_helper/api/v1/operator/summary",
    )
    .await?;

    let runtime_status = get_json::<RuntimeStatusResponse>(
        &client,
        &endpoint.admin_base_url,
        link_or_default(
            &operator_summary,
            "runtime_status",
            "/__codex_helper/api/v1/runtime/status",
        ),
    )
    .await
    .ok();

    let providers = get_json::<Vec<Value>>(
        &client,
        &endpoint.admin_base_url,
        link_or_default(
            &operator_summary,
            "providers",
            "/__codex_helper/api/v1/providers",
        ),
    )
    .await
    .unwrap_or_default();

    let recent_requests = get_json::<Vec<Value>>(
        &client,
        &endpoint.admin_base_url,
        &format!(
            "{}?limit=40",
            link_or_default(
                &operator_summary,
                "request_ledger_recent",
                "/__codex_helper/api/v1/request-ledger/recent"
            )
        ),
    )
    .await
    .unwrap_or_default();

    let usage_summary = get_json::<Vec<Value>>(
        &client,
        &endpoint.admin_base_url,
        &format!(
            "{}?by=provider&limit=30",
            link_or_default(
                &operator_summary,
                "request_ledger_summary",
                "/__codex_helper/api/v1/request-ledger/summary"
            )
        ),
    )
    .await
    .unwrap_or_default();

    Ok(AdminReadModel {
        endpoint,
        operator_summary,
        runtime_status,
        providers,
        recent_requests,
        usage_summary,
    })
}

pub(crate) fn admin_client() -> Result<reqwest::Client, CommandError> {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(REQUEST_TIMEOUT_MS))
        .build()
        .map_err(|err| DesktopError::AdminApi(err.to_string()).into())
}

pub(crate) async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
) -> Result<T, CommandError> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = with_admin_headers(
        client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json"),
    )
    .send()
    .await
    .map_err(|err| DesktopError::AdminApi(format!("{url}: {err}")))?;
    decode_response(response, &url).await
}

pub(crate) async fn post_json<T: DeserializeOwned, B: Serialize + ?Sized>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    body: &B,
) -> Result<T, CommandError> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = with_admin_headers(
        client
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(body),
    )
    .send()
    .await
    .map_err(|err| DesktopError::AdminApi(format!("{url}: {err}")))?;
    decode_response(response, &url).await
}

pub(crate) async fn post_empty(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
) -> Result<(), CommandError> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = with_admin_headers(
        client
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json"),
    )
    .send()
    .await
    .map_err(|err| DesktopError::AdminApi(format!("{url}: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(DesktopError::AdminApi(format!("{url}: HTTP {status} {body}")).into());
    }
    Ok(())
}

pub(crate) async fn post_json_no_response<B: Serialize + ?Sized>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    body: &B,
) -> Result<(), CommandError> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = with_admin_headers(
        client
            .post(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(body),
    )
    .send()
    .await
    .map_err(|err| DesktopError::AdminApi(format!("{url}: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(DesktopError::AdminApi(format!("{url}: HTTP {status} {body}")).into());
    }
    Ok(())
}

async fn decode_response<T: DeserializeOwned>(
    response: reqwest::Response,
    url: &str,
) -> Result<T, CommandError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(DesktopError::AdminApi(format!("{url}: HTTP {status} {body}")).into());
    }
    response
        .json::<T>()
        .await
        .map_err(|err| DesktopError::AdminApi(format!("{url}: {err}")).into())
}

fn with_admin_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Some(token) = std::env::var(ADMIN_TOKEN_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        request.header(ADMIN_TOKEN_HEADER, token)
    } else {
        request
    }
}

pub(crate) fn admin_endpoint_config() -> AdminEndpointConfig {
    if let Ok(base) = std::env::var(ADMIN_BASE_ENV) {
        if let Some(config) = config_from_admin_base_url(base.trim()) {
            return config;
        }
    }

    AdminEndpointConfig {
        proxy_port: DEFAULT_PROXY_PORT,
        admin_port: admin_port_for_proxy_port(DEFAULT_PROXY_PORT),
        proxy_base_url: local_proxy_base_url(DEFAULT_PROXY_PORT),
        admin_base_url: local_admin_base_url_for_proxy_port(DEFAULT_PROXY_PORT),
    }
}

fn config_from_admin_base_url(value: &str) -> Option<AdminEndpointConfig> {
    let url = reqwest::Url::parse(value).ok()?;
    let admin_port = url.port_or_known_default()?;
    let proxy_port = if admin_port > ADMIN_PORT_OFFSET {
        admin_port.saturating_sub(ADMIN_PORT_OFFSET)
    } else {
        DEFAULT_PROXY_PORT
    };
    Some(AdminEndpointConfig {
        proxy_port,
        admin_port,
        proxy_base_url: local_proxy_base_url(proxy_port),
        admin_base_url: value.trim_end_matches('/').to_string(),
    })
}

fn link_or_default<'a>(summary: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    summary
        .get("links")
        .and_then(|links| links.get(key))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}
