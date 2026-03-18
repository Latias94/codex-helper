use std::time::Duration;

use anyhow::bail;

use super::super::super::{ProxyController, send_admin_request};
use super::super::{mode_child_control_url, mode_template_control_url, refresh_now};

impl ProxyController {
    #[allow(dead_code)]
    pub fn set_persisted_active_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        let url = mode_child_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_persisted_station_settings,
            "attached proxy does not support persisted station settings (need api v1)",
            |links| Some(links.stations.as_str()),
            "/__codex_helper/api/v1/stations",
            "active",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.post(url).timeout(Duration::from_millis(1200)).json(
                &serde_json::json!({
                    "station_name": station_name,
                }),
            ))
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
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

        let url = mode_template_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_persisted_station_settings,
            "attached proxy does not support persisted station settings (need api v1)",
            |links| Some(links.station_by_name_template.as_str()),
            "/__codex_helper/api/v1/stations/{name}",
            "{name}",
            station_name.trim(),
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.put(url).timeout(Duration::from_millis(1200)).json(
                &serde_json::json!({
                    "enabled": enabled,
                    "level": level,
                }),
            ))
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

        let url = mode_template_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_station_spec_api,
            "attached proxy does not support persisted station spec API (need api v1)",
            |links| Some(links.station_spec_by_name_template.as_str()),
            "/__codex_helper/api/v1/stations/specs/{name}",
            "{name}",
            station_name.trim(),
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.put(url).timeout(Duration::from_millis(1500)).json(
                &serde_json::json!({
                    "alias": station.alias,
                    "enabled": station.enabled,
                    "level": station.level,
                    "members": station.members,
                }),
            ))
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

        let url = mode_template_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_station_spec_api,
            "attached proxy does not support persisted station spec API (need api v1)",
            |links| Some(links.station_spec_by_name_template.as_str()),
            "/__codex_helper/api/v1/stations/specs/{name}",
            "{name}",
            station_name.trim(),
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.delete(url).timeout(Duration::from_millis(1500))).await?;
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

        let url = mode_template_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_provider_spec_api,
            "attached proxy does not support persisted provider spec API (need api v1)",
            |links| Some(links.provider_spec_by_name_template.as_str()),
            "/__codex_helper/api/v1/providers/specs/{name}",
            "{name}",
            provider_name.trim(),
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.put(url).timeout(Duration::from_millis(1500)).json(
                &serde_json::json!({
                    "alias": provider.alias,
                    "enabled": provider.enabled,
                    "auth_token_env": provider.auth_token_env,
                    "api_key_env": provider.api_key_env,
                    "endpoints": provider.endpoints,
                }),
            ))
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

        let url = mode_template_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_provider_spec_api,
            "attached proxy does not support persisted provider spec API (need api v1)",
            |links| Some(links.provider_spec_by_name_template.as_str()),
            "/__codex_helper/api/v1/providers/specs/{name}",
            "{name}",
            provider_name.trim(),
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.delete(url).timeout(Duration::from_millis(1500))).await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }
}
