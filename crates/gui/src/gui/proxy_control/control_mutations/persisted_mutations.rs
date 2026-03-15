use std::time::Duration;

use anyhow::bail;

use crate::config::{ResolvedRetryConfig, RetryConfig, ServiceControlProfile};

use super::super::{ProxyController, ProxyMode, send_admin_request};
use super::{mode_control_base, refresh_now};

impl ProxyController {
    #[allow(dead_code)]
    pub fn set_persisted_default_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: Option<String>,
    ) -> anyhow::Result<()> {
        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support persisted profile config (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!(
                        "{base}/__codex_helper/api/v1/profiles/default/persisted"
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&serde_json::json!({
                        "profile_name": profile_name,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_persisted_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: String,
        profile: ServiceControlProfile,
    ) -> anyhow::Result<()> {
        if profile_name.trim().is_empty() {
            bail!("profile name is required");
        }

        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support persisted profile config (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/profiles/{}",
                        profile_name.trim()
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&profile),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_persisted_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: String,
    ) -> anyhow::Result<()> {
        if profile_name.trim().is_empty() {
            bail!("profile name is required");
        }

        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support persisted profile config (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .delete(format!(
                        "{base}/__codex_helper/api/v1/profiles/{}",
                        profile_name.trim()
                    ))
                    .timeout(Duration::from_millis(1200)),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_persisted_active_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_persisted_station_config,
            "attached proxy does not support persisted station config (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!(
                        "{base}/__codex_helper/api/v1/stations/config-active"
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&serde_json::json!({
                        "station_name": station_name,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_persisted_retry_config(
        &mut self,
        rt: &tokio::runtime::Runtime,
        retry: RetryConfig,
    ) -> anyhow::Result<()> {
        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_retry_config_api,
            "attached proxy does not support persisted retry config (need api v1)",
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
                    .post(format!("{base}/__codex_helper/api/v1/retry/config"))
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

        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn update_persisted_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
        enabled: bool,
        level: u8,
    ) -> anyhow::Result<()> {
        if station_name.trim().is_empty() {
            bail!("station name is required");
        }

        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_persisted_station_config,
            "attached proxy does not support persisted station config (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/stations/{}",
                        station_name.trim()
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&serde_json::json!({
                        "enabled": enabled,
                        "level": level,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_persisted_station_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
        station: crate::config::PersistedStationSpec,
    ) -> anyhow::Result<()> {
        if station_name.trim().is_empty() {
            bail!("station name is required");
        }

        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_station_spec_api,
            "attached proxy does not support persisted station spec API (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/stations/specs/{}",
                        station_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500))
                    .json(&serde_json::json!({
                        "alias": station.alias,
                        "enabled": station.enabled,
                        "level": station.level,
                        "members": station.members,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_persisted_station_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
    ) -> anyhow::Result<()> {
        if station_name.trim().is_empty() {
            bail!("station name is required");
        }

        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_station_spec_api,
            "attached proxy does not support persisted station spec API (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .delete(format!(
                        "{base}/__codex_helper/api/v1/stations/specs/{}",
                        station_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500)),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_persisted_provider_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        provider_name: String,
        provider: crate::config::PersistedProviderSpec,
    ) -> anyhow::Result<()> {
        if provider_name.trim().is_empty() {
            bail!("provider name is required");
        }

        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_provider_spec_api,
            "attached proxy does not support persisted provider spec API (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/providers/specs/{}",
                        provider_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500))
                    .json(&serde_json::json!({
                        "alias": provider.alias,
                        "enabled": provider.enabled,
                        "auth_token_env": provider.auth_token_env,
                        "api_key_env": provider.api_key_env,
                        "endpoints": provider.endpoints,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_persisted_provider_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        provider_name: String,
    ) -> anyhow::Result<()> {
        if provider_name.trim().is_empty() {
            bail!("provider name is required");
        }

        let base = mode_control_base(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_provider_spec_api,
            "attached proxy does not support persisted provider spec API (need api v1)",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .delete(format!(
                        "{base}/__codex_helper/api/v1/providers/specs/{}",
                        provider_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500)),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }
}
