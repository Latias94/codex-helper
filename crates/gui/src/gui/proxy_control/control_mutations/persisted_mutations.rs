mod profile_persistence;

use std::time::Duration;

use crate::config::{ResolvedRetryConfig, RetryConfig};

use super::super::{ProxyController, ProxyMode, send_admin_request};
use super::{mode_control_url, refresh_now};

impl ProxyController {
    #[allow(dead_code)]
    pub fn set_persisted_retry_config(
        &mut self,
        rt: &tokio::runtime::Runtime,
        retry: RetryConfig,
    ) -> anyhow::Result<()> {
        let url = mode_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_retry_config_api,
            "attached proxy does not support persisted retry config (need api v1)",
            |links| Some(links.retry_config.as_str()),
            "/__codex_helper/api/v1/retry/config",
        )?;

        #[derive(serde::Deserialize)]
        struct RetryConfigResponse {
            configured: RetryConfig,
            resolved: ResolvedRetryConfig,
        }

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(url)
                    .timeout(Duration::from_millis(1200))
                    .json(&retry),
            )
            .await?
            .json::<RetryConfigResponse>()
            .await
            .map_err(anyhow::Error::from)
        };
        let response = rt.block_on(fut)?;

        match &mut self.mode {
            ProxyMode::Running(r) => {
                r.configured_retry = Some(response.configured.clone());
                r.resolved_retry = Some(response.resolved.clone());
            }
            ProxyMode::Attached(att) => {
                att.configured_retry = Some(response.configured.clone());
                att.resolved_retry = Some(response.resolved.clone());
                att.supports_retry_config_api = true;
            }
            _ => {}
        }

        refresh_now(self, rt)?;
        Ok(())
    }
}
