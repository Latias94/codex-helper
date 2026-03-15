use std::time::Duration;

use anyhow::bail;

use crate::config::ServiceControlProfile;

use super::super::super::{ProxyController, send_admin_request};
use super::super::{mode_control_base, refresh_now};

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
}
