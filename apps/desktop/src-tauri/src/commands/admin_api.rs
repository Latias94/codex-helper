use std::time::Duration;

use codex_helper_core::proxy::{
    ADMIN_PORT_OFFSET, ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN_HEADER, RuntimeStatusResponse,
    admin_port_for_proxy_port, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::CommandError;

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
    pub code: Option<String>,
    pub error: Option<String>,
}

impl AdminReadModelSectionStatus {
    fn ok(section: impl Into<String>) -> Self {
        Self {
            section: section.into(),
            ok: true,
            code: None,
            error: None,
        }
    }

    fn error(
        section: impl Into<String>,
        code: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            section: section.into(),
            ok: false,
            code: Some(code.into()),
            error: Some(error.into()),
        }
    }

    fn command_error(section: impl Into<String>, error: CommandError) -> Self {
        Self {
            section: section.into(),
            ok: false,
            code: Some(error.code),
            error: Some(error.message),
        }
    }
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
                    "desktop_admin_missing_usage_day",
                    "snapshot response did not include snapshot.usage_day",
                ));
            }
            usage_day
        }
        Err(err) => {
            section_statuses.push(AdminReadModelSectionStatus::command_error("usageDay", err));
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

#[tauri::command]
pub async fn get_request_chain(payload: RequestChainPayload) -> Result<Value, CommandError> {
    if !request_chain_payload_has_selector(&payload) {
        return Err(CommandError::new(
            "desktop_request_chain_selector_required",
            "traceId, requestId, or session is required",
            false,
        ));
    }
    let endpoint = admin_endpoint_config();
    let client = admin_client()?;
    let path = request_chain_path(&payload);
    get_json::<Value>(&client, &endpoint.admin_base_url, &path).await
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
            statuses.push(AdminReadModelSectionStatus::command_error(section, err));
            None
        }
    }
}

pub(crate) fn admin_client() -> Result<reqwest::Client, CommandError> {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(REQUEST_TIMEOUT_MS))
        .build()
        .map_err(|err| {
            CommandError::new(
                "desktop_admin_client_error",
                format!("failed to build admin API client: {err}"),
                false,
            )
        })
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
    .map_err(|err| admin_request_error(&url, err))?;
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
    .map_err(|err| admin_request_error(&url, err))?;
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
    .map_err(|err| admin_request_error(&url, err))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(admin_status_error(&url, status, body));
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
    .map_err(|err| admin_request_error(&url, err))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(admin_status_error(&url, status, body));
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
        return Err(admin_status_error(url, status, body));
    }
    response.json::<T>().await.map_err(|err| {
        CommandError::new(
            "desktop_admin_decode_error",
            format!("admin API {url}: decode JSON response failed: {err}"),
            true,
        )
    })
}

fn admin_request_error(url: &str, err: reqwest::Error) -> CommandError {
    let code = if err.is_timeout() {
        "desktop_admin_timeout"
    } else if err.is_connect() {
        "desktop_admin_connection_failed"
    } else {
        "desktop_admin_request_failed"
    };
    CommandError::new(code, format!("admin API {url}: {err}"), true)
}

fn admin_status_error(url: &str, status: reqwest::StatusCode, body: String) -> CommandError {
    let code = match status.as_u16() {
        401 => "desktop_admin_http_401",
        403 => "desktop_admin_http_403",
        429 => "desktop_admin_http_429",
        _ => "desktop_admin_http_status",
    };
    let retryable = status.is_server_error() || status.as_u16() == 429;
    let error = CommandError::new(
        code,
        format!("admin API {url}: HTTP {status} {body}"),
        retryable,
    );
    if matches!(status.as_u16(), 401 | 403) {
        error.with_hint("set CODEX_HELPER_ADMIN_TOKEN for the desktop process")
    } else {
        error
    }
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

fn request_chain_path(payload: &RequestChainPayload) -> String {
    let mut url =
        reqwest::Url::parse("http://localhost/__codex_helper/api/v1/request-ledger/chain")
            .expect("request chain URL literal should be valid");
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("limit", &payload.limit.unwrap_or(20).to_string());
        if let Some(trace_id) = payload
            .trace_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            query.append_pair("trace_id", trace_id);
        }
        if let Some(request_id) = payload.request_id {
            query.append_pair("request_id", &request_id.to_string());
        }
        if let Some(session) = payload
            .session
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            query.append_pair("session", session);
        }
    }
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
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
    fn request_chain_path_encodes_selector() {
        let payload = RequestChainPayload {
            trace_id: Some(" trace/with space ".to_string()),
            request_id: Some(42),
            session: Some("session a".to_string()),
            limit: Some(10),
        };

        assert!(request_chain_payload_has_selector(&payload));
        assert_eq!(
            request_chain_path(&payload),
            "/__codex_helper/api/v1/request-ledger/chain?limit=10&trace_id=trace%2Fwith+space&request_id=42&session=session+a"
        );
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
    fn optional_section_statuses_distinguish_empty_from_failed() {
        let mut statuses = Vec::new();
        let ok = record_optional_section::<Vec<Value>>(&mut statuses, "providers", Ok(Vec::new()));
        assert_eq!(ok, Some(Vec::new()));
        assert!(statuses[0].ok);
        assert_eq!(statuses[0].section, "providers");

        let failed = record_optional_section::<Vec<Value>>(
            &mut statuses,
            "usageDay",
            Err(CommandError::new(
                "desktop_admin_connection_failed",
                "admin API unavailable",
                true,
            )),
        );
        assert!(failed.is_none());
        assert!(!statuses[1].ok);
        assert_eq!(statuses[1].section, "usageDay");
        assert_eq!(
            statuses[1].code.as_deref(),
            Some("desktop_admin_connection_failed")
        );
        assert_eq!(statuses[1].error.as_deref(), Some("admin API unavailable"));
    }

    #[test]
    fn admin_status_error_classifies_auth_failures() {
        let err = admin_status_error(
            "http://127.0.0.1:4211/__codex_helper/api/v1/providers",
            reqwest::StatusCode::FORBIDDEN,
            "forbidden".to_string(),
        );
        let value = serde_json::to_value(&err).expect("serialize admin status error");

        assert_eq!(value["code"].as_str(), Some("desktop_admin_http_403"));
        assert_eq!(value["retryable"].as_bool(), Some(false));
        assert_eq!(
            value["hint"].as_str(),
            Some("set CODEX_HELPER_ADMIN_TOKEN for the desktop process")
        );
    }
}
