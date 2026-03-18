use std::time::Duration;

use anyhow::bail;

use crate::config::ServiceControlProfile;

use super::super::super::{ProxyController, send_admin_request};
use super::super::{mode_control_url, mode_template_control_url, refresh_now};

impl ProxyController {
    #[allow(dead_code)]
    pub fn set_persisted_default_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: Option<String>,
    ) -> anyhow::Result<()> {
        let url = mode_control_url(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support persisted profile config (need api v1)",
            |links| Some(links.persisted_default_profile.as_str()),
            "/__codex_helper/api/v1/profiles/default/persisted",
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.post(url).timeout(Duration::from_millis(1200)).json(
                &serde_json::json!({
                    "profile_name": profile_name,
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
    pub fn upsert_persisted_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: String,
        profile: ServiceControlProfile,
    ) -> anyhow::Result<()> {
        if profile_name.trim().is_empty() {
            bail!("profile name is required");
        }

        let url = mode_template_control_url(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support persisted profile config (need api v1)",
            |links| Some(links.profile_by_name_template.as_str()),
            "/__codex_helper/api/v1/profiles/{name}",
            "{name}",
            profile_name.trim(),
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(url)
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

        let url = mode_template_control_url(
            &self.mode,
            |att| att.api_version == Some(1),
            "attached proxy does not support persisted profile config (need api v1)",
            |links| Some(links.profile_by_name_template.as_str()),
            "/__codex_helper/api/v1/profiles/{name}",
            "{name}",
            profile_name.trim(),
        )?;

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(client.delete(url).timeout(Duration::from_millis(1200))).await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        refresh_now(self, rt);
        Ok(())
    }
}
