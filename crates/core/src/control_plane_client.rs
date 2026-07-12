use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::{Client, Method, Url};
use serde::Deserialize;
use thiserror::Error;

use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1OperatorSummary, ApiV1Snapshot, ProviderOption,
};
use crate::fleet::FleetSnapshot;
use crate::proxy::{ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN_HEADER, RuntimeStatusResponse};
use crate::request_chain::{RequestChainExport, RequestChainSelector};

const ADMIN_DISCOVERY_PATH: &str = "/.well-known/codex-helper-admin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneEndpoint {
    pub admin_base_url: String,
    pub admin_token_env: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ControlPlaneClient {
    endpoint: ControlPlaneEndpoint,
    client: Client,
}

#[derive(Debug, Error)]
pub enum ControlPlaneError {
    #[error("admin API not reachable at {base_url}: {source}")]
    Transport {
        base_url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("admin API returned {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("admin API response is not valid JSON: {source}")]
    Decode {
        #[source]
        source: reqwest::Error,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminDiscoveryDocument {
    pub api_version: u32,
    pub service_name: String,
    pub admin_base_url: String,
}

impl ControlPlaneEndpoint {
    pub fn new(
        admin_base_url: impl Into<String>,
        admin_token_env: Option<impl Into<String>>,
    ) -> Result<Self> {
        let admin_base_url = normalize_base_url(&admin_base_url.into())
            .ok_or_else(|| anyhow!("admin base URL is required"))?;
        Ok(Self {
            admin_base_url,
            admin_token_env: admin_token_env.map(Into::into),
        })
    }
}

impl ControlPlaneClient {
    pub fn new(endpoint: ControlPlaneEndpoint) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(1200))
            .build()
            .context("failed to build control-plane client")?;
        Ok(Self { endpoint, client })
    }

    pub fn endpoint(&self) -> &ControlPlaneEndpoint {
        &self.endpoint
    }

    pub async fn discover_admin_base_url(proxy_base_url: &str) -> Result<AdminDiscoveryDocument> {
        let proxy_base_url = normalize_base_url(proxy_base_url)
            .ok_or_else(|| anyhow!("proxy base URL is required"))?;
        let url = format!("{proxy_base_url}{ADMIN_DISCOVERY_PATH}");
        let response = Client::builder()
            .timeout(Duration::from_millis(1200))
            .build()
            .context("failed to build discovery client")?
            .get(url)
            .send()
            .await
            .context("admin discovery request failed")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("admin discovery returned {status}: {}", body.trim());
        }
        response
            .json::<AdminDiscoveryDocument>()
            .await
            .context("admin discovery response is not valid JSON")
    }

    pub async fn fetch_json<T>(&self, path: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.fetch_json_classified(path).await.map_err(Into::into)
    }

    pub async fn fetch_json_classified<T>(&self, path: &str) -> Result<T, ControlPlaneError>
    where
        T: serde::de::DeserializeOwned,
    {
        self.request_json_classified(Method::GET, path).await
    }

    pub async fn post_json<T>(&self, path: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.request_json_classified(Method::POST, path)
            .await
            .map_err(Into::into)
    }

    async fn request_json_classified<T>(
        &self,
        method: Method,
        path: &str,
    ) -> Result<T, ControlPlaneError>
    where
        T: serde::de::DeserializeOwned,
    {
        let url = format!("{}{}", self.endpoint.admin_base_url, path);
        let mut request = self.client.request(method, url);
        if let Some(token) = self.admin_token() {
            request = request.header(ADMIN_TOKEN_HEADER, token);
        }

        let response = request
            .send()
            .await
            .map_err(|source| ControlPlaneError::Transport {
                base_url: self.endpoint.admin_base_url.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ControlPlaneError::HttpStatus {
                status: status.as_u16(),
                body: body.trim().to_string(),
            });
        }
        response
            .json::<T>()
            .await
            .map_err(|source| ControlPlaneError::Decode { source })
    }

    pub async fn runtime_status(&self) -> Result<RuntimeStatusResponse> {
        self.fetch_json("/__codex_helper/api/v1/runtime/status")
            .await
    }

    pub async fn capabilities(&self) -> Result<ApiV1Capabilities> {
        self.fetch_json("/__codex_helper/api/v1/capabilities").await
    }

    pub async fn operator_summary(&self) -> Result<ApiV1OperatorSummary> {
        self.fetch_json("/__codex_helper/api/v1/operator/summary")
            .await
    }

    pub async fn snapshot(&self, recent_limit: usize, stats_days: usize) -> Result<ApiV1Snapshot> {
        self.fetch_json(&format!(
            "/__codex_helper/api/v1/snapshot?recent_limit={recent_limit}&stats_days={stats_days}"
        ))
        .await
    }

    pub async fn fleet_snapshot(&self) -> Result<FleetSnapshot, ControlPlaneError> {
        self.fetch_json_classified("/__codex_helper/api/v1/fleet/snapshot")
            .await
    }

    pub async fn request_chain(
        &self,
        selector: RequestChainSelector,
        limit: usize,
    ) -> Result<RequestChainExport> {
        self.fetch_json(&request_chain_path(selector, limit)).await
    }

    pub async fn providers(&self) -> Result<Vec<ProviderOption>> {
        self.fetch_json("/__codex_helper/api/v1/providers").await
    }

    fn admin_token(&self) -> Option<String> {
        let env_name = self
            .endpoint
            .admin_token_env
            .as_deref()
            .unwrap_or(ADMIN_TOKEN_ENV_VAR);
        std::env::var(env_name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }
}

pub fn normalize_base_url(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return None;
    }
    let url = Url::parse(value).ok()?;
    match url.scheme() {
        "http" | "https" => Some(value.to_string()),
        _ => None,
    }
}

fn request_chain_path(selector: RequestChainSelector, limit: usize) -> String {
    let selector = selector.normalized();
    let mut url =
        Url::parse("http://localhost/__codex_helper/api/v1/request-ledger/chain").expect("url");
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("limit", &limit.to_string());
        if let Some(trace_id) = selector.trace_id {
            pairs.append_pair("trace_id", &trace_id);
        }
        if let Some(request_id) = selector.request_id {
            pairs.append_pair("request_id", &request_id.to_string());
        }
        if let Some(session_id) = selector.session_id {
            pairs.append_pair("session", &session_id);
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
    fn control_plane_endpoint_normalizes_admin_base_url() {
        let endpoint = ControlPlaneEndpoint::new(" http://nas.local:4211/ ", Some("TOKEN_ENV"))
            .expect("endpoint");

        assert_eq!(endpoint.admin_base_url, "http://nas.local:4211");
        assert_eq!(endpoint.admin_token_env.as_deref(), Some("TOKEN_ENV"));
    }

    #[test]
    fn control_plane_endpoint_rejects_non_http_url() {
        let err = ControlPlaneEndpoint::new("file:///tmp/socket", None::<String>)
            .expect_err("non-http admin url should fail");

        assert!(err.to_string().contains("admin base URL"));
    }

    #[test]
    fn request_chain_path_encodes_selector_query_values() {
        let path = request_chain_path(
            RequestChainSelector {
                trace_id: Some("trace/with space".to_string()),
                request_id: Some(42),
                session_id: Some("session a".to_string()),
            },
            20,
        );

        assert_eq!(
            path,
            "/__codex_helper/api/v1/request-ledger/chain?limit=20&trace_id=trace%2Fwith+space&request_id=42&session=session+a"
        );
    }
}
