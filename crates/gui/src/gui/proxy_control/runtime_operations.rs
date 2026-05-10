use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;

use crate::config::load_config;

use super::control_mutations::mode_control_url;
use super::{ProxyController, ProxyMode, send_admin_request};

impl ProxyController {
    pub fn sync_running_config_from_disk(
        &mut self,
        rt: &tokio::runtime::Runtime,
    ) -> anyhow::Result<bool> {
        let ProxyMode::Running(r) = &mut self.mode else {
            return Ok(false);
        };

        let cfg = rt.block_on(load_config())?;
        r.cfg = Arc::new(cfg);
        Ok(true)
    }

    pub fn reload_runtime_config(&mut self, rt: &tokio::runtime::Runtime) -> anyhow::Result<()> {
        let url = mode_control_url(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support runtime reload (need api v1)",
            |links| Some(links.runtime_reload.as_str()),
            "/__codex_helper/api/v1/runtime/reload",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.post(url).timeout(Duration::from_millis(800))).await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.sync_running_config_from_disk(rt)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn start_health_checks(
        &mut self,
        rt: &tokio::runtime::Runtime,
        all: bool,
        station_names: Vec<String>,
    ) -> anyhow::Result<()> {
        let url = mode_control_url(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support health checks (need api v1)",
            |links| Some(links.healthcheck_start.as_str()),
            "/__codex_helper/api/v1/healthcheck/start",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(url)
                    .timeout(Duration::from_millis(800))
                    .json(&serde_json::json!({ "all": all, "station_names": station_names })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn probe_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
    ) -> anyhow::Result<()> {
        let station_name = station_name.trim().to_string();
        if station_name.is_empty() {
            bail!("station_name cannot be empty");
        }

        let use_station_api = match &self.mode {
            ProxyMode::Running(_) => true,
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support manual probes (need api v1)");
                }
                a.supports_station_api
            }
            _ => bail!("proxy is not running/attached"),
        };

        if !use_station_api {
            return self.start_health_checks(rt, false, vec![station_name]);
        }

        let url = mode_control_url(
            &self.mode,
            |att| att.api_version == Some(1) && att.supports_station_api,
            "attached proxy does not support manual probes (need api v1)",
            |links| Some(links.station_probe.as_str()),
            "/__codex_helper/api/v1/stations/probe",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(url)
                    .timeout(Duration::from_millis(800))
                    .json(&serde_json::json!({ "station_name": station_name })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn cancel_health_checks(
        &mut self,
        rt: &tokio::runtime::Runtime,
        all: bool,
        station_names: Vec<String>,
    ) -> anyhow::Result<()> {
        let url = mode_control_url(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support health checks (need api v1)",
            |links| Some(links.healthcheck_cancel.as_str()),
            "/__codex_helper/api/v1/healthcheck/cancel",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(url)
                    .timeout(Duration::from_millis(800))
                    .json(&serde_json::json!({ "all": all, "station_names": station_names })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }
}
