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
    pub usage_day: Option<Value>,
    pub section_statuses: Vec<AdminReadModelSectionStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminReadModelSectionStatus {
    pub section: String,
    pub ok: bool,
    pub error: Option<String>,
}

impl AdminReadModelSectionStatus {
    fn ok(section: impl Into<String>) -> Self {
        Self {
            section: section.into(),
            ok: true,
            error: None,
        }
    }

    fn error(section: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            section: section.into(),
            ok: false,
            error: Some(error.into()),
        }
    }
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
    let mut section_statuses = vec![AdminReadModelSectionStatus::ok("operatorSummary")];

    let runtime_status = record_optional_section(
        &mut section_statuses,
        "runtimeStatus",
        get_json::<RuntimeStatusResponse>(
            &client,
            &endpoint.admin_base_url,
            link_or_default(
                &operator_summary,
                "runtime_status",
                "/__codex_helper/api/v1/runtime/status",
            ),
        )
        .await,
    );

    let providers = record_optional_section(
        &mut section_statuses,
        "providers",
        get_json::<Vec<Value>>(
            &client,
            &endpoint.admin_base_url,
            link_or_default(
                &operator_summary,
                "providers",
                "/__codex_helper/api/v1/providers",
            ),
        )
        .await,
    )
    .unwrap_or_default();

    let recent_requests = record_optional_section(
        &mut section_statuses,
        "recentRequests",
        get_json::<Vec<Value>>(
            &client,
            &endpoint.admin_base_url,
            &append_query(
                link_or_default(
                    &operator_summary,
                    "request_ledger_recent",
                    "/__codex_helper/api/v1/request-ledger/recent",
                ),
                "limit=40",
            ),
        )
        .await,
    )
    .unwrap_or_default();

    let usage_summary = record_optional_section(
        &mut section_statuses,
        "usageSummary",
        get_json::<Vec<Value>>(
            &client,
            &endpoint.admin_base_url,
            &append_query(
                link_or_default(
                    &operator_summary,
                    "request_ledger_summary",
                    "/__codex_helper/api/v1/request-ledger/summary",
                ),
                "by=provider&limit=30",
            ),
        )
        .await,
    )
    .unwrap_or_default();

    let usage_day = match get_json::<Value>(
        &client,
        &endpoint.admin_base_url,
        &append_query(
            link_or_default(
                &operator_summary,
                "snapshot",
                "/__codex_helper/api/v1/snapshot",
            ),
            "recent_limit=40&stats_days=1",
        ),
    )
    .await
    {
        Ok(snapshot) => {
            let usage_day = snapshot
                .get("snapshot")
                .and_then(|snapshot| snapshot.get("usage_day"))
                .cloned();
            if usage_day.is_some() {
                section_statuses.push(AdminReadModelSectionStatus::ok("usageDay"));
            } else {
                section_statuses.push(AdminReadModelSectionStatus::error(
                    "usageDay",
                    "snapshot response did not include snapshot.usage_day",
                ));
            }
            usage_day
        }
        Err(err) => {
            section_statuses.push(AdminReadModelSectionStatus::error("usageDay", err.message));
            None
        }
    };

    Ok(AdminReadModel {
        endpoint,
        operator_summary,
        runtime_status,
        providers,
        recent_requests,
        usage_summary,
        usage_day,
        section_statuses,
    })
}

fn record_optional_section<T>(
    statuses: &mut Vec<AdminReadModelSectionStatus>,
    section: &str,
    result: Result<T, CommandError>,
) -> Option<T> {
    match result {
        Ok(value) => {
            statuses.push(AdminReadModelSectionStatus::ok(section));
            Some(value)
        }
        Err(err) => {
            statuses.push(AdminReadModelSectionStatus::error(section, err.message));
            None
        }
    }
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
    if let Ok(base) = std::env::var(ADMIN_BASE_ENV)
        && let Some(config) = config_from_admin_base_url(base.trim())
    {
        return config;
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

fn append_query(path: &str, query: &str) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}{query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_query_preserves_existing_query_string() {
        assert_eq!(
            append_query("/snapshot", "stats_days=1"),
            "/snapshot?stats_days=1"
        );
        assert_eq!(
            append_query("/snapshot?recent_limit=40", "stats_days=1"),
            "/snapshot?recent_limit=40&stats_days=1"
        );
    }

    #[test]
    fn optional_section_statuses_distinguish_empty_from_failed() {
        let mut statuses = Vec::new();
        let ok = record_optional_section::<Vec<Value>>(&mut statuses, "providers", Ok(Vec::new()));
        assert_eq!(ok, Some(Vec::new()));
        assert!(statuses[0].ok);
        assert_eq!(statuses[0].section, "providers");

        let failed = record_optional_section::<Vec<Value>>(
            &mut statuses,
            "usageDay",
            Err(CommandError {
                message: "admin API unavailable".to_string(),
            }),
        );
        assert!(failed.is_none());
        assert!(!statuses[1].ok);
        assert_eq!(statuses[1].section, "usageDay");
        assert_eq!(statuses[1].error.as_deref(), Some("admin API unavailable"));
    }
}
