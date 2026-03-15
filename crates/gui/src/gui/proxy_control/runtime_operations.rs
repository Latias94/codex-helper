use std::time::Duration;

use anyhow::bail;

use crate::proxy::local_proxy_base_url;

use super::{ProxyController, ProxyMode, send_admin_request};

impl ProxyController {
    pub fn reload_runtime_config(&mut self, rt: &tokio::runtime::Runtime) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support runtime reload (need api v1)");
                }
                a.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            let url = format!("{base}/__codex_helper/api/v1/runtime/reload");
            send_admin_request(client.post(url).timeout(Duration::from_millis(800))).await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn start_health_checks(
        &mut self,
        rt: &tokio::runtime::Runtime,
        all: bool,
        station_names: Vec<String>,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support health checks (need api v1)");
                }
                a.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!("{base}/__codex_helper/api/v1/healthcheck/start"))
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

        let (base, use_station_api) = match &self.mode {
            ProxyMode::Running(r) => (local_proxy_base_url(r.admin_port), true),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support manual probes (need api v1)");
                }
                (a.admin_base_url.clone(), a.supports_station_api)
            }
            _ => bail!("proxy is not running/attached"),
        };

        if !use_station_api {
            return self.start_health_checks(rt, false, vec![station_name]);
        }

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!("{base}/__codex_helper/api/v1/stations/probe"))
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
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support health checks (need api v1)");
                }
                a.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!("{base}/__codex_helper/api/v1/healthcheck/cancel"))
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
