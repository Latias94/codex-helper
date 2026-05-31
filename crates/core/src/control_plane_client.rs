use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::{Client, Url};
use serde::Deserialize;

use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1OperatorSummary, ApiV1Snapshot, ProviderOption,
};
use crate::proxy::{ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN_HEADER, RuntimeStatusResponse};

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
        let url = format!("{}{}", self.endpoint.admin_base_url, path);
        let mut request = self.client.get(url);
        if let Some(token) = self.admin_token() {
            request = request.header(ADMIN_TOKEN_HEADER, token);
        }

        let response = request.send().await.with_context(|| {
            format!(
                "admin API not reachable at {}",
                self.endpoint.admin_base_url
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("admin API returned {status}: {}", body.trim());
        }
        response
            .json::<T>()
            .await
            .context("admin API response is not valid JSON")
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
}
