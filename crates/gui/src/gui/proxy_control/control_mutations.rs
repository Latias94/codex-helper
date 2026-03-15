use std::time::Duration;

use anyhow::bail;

use crate::config::{ResolvedRetryConfig, RetryConfig, ServiceControlProfile};
use crate::proxy::local_proxy_base_url;
use crate::state::RuntimeConfigState;

use super::running_refresh::{
    effective_default_profile_from_cfg_state, effective_stations_from_cfg_state,
    list_profiles_from_cfg,
};
use super::{ProxyController, ProxyMode, now_ms, send_admin_request};

impl ProxyController {
    pub fn apply_session_effort_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        effort: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match effort {
                        Some(eff) => {
                            state
                                .set_session_effort_override(session_id, eff, now)
                                .await
                        }
                        None => state.clear_session_effort_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support session effort overrides (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    let payload = serde_json::json!({
                        "session_id": session_id,
                        "effort": effort,
                    });
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/effort"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&payload),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_model_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        model: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match model {
                        Some(model) => {
                            state
                                .set_session_model_override(session_id, model, now)
                                .await
                        }
                        None => state.clear_session_model_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support session model overrides (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/model"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "model": model,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        profile_name: String,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let service_name = r.service_name;
                let cfg = r.cfg.clone();
                let now = now_ms();
                rt.block_on(async move {
                    let mgr = match service_name {
                        "claude" => &cfg.claude,
                        _ => &cfg.codex,
                    };
                    state
                        .apply_session_profile_binding(
                            service_name,
                            mgr,
                            session_id,
                            profile_name,
                            now,
                        )
                        .await
                })?;
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support session profile apply (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/profile"
                            ))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "profile_name": profile_name,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn clear_session_profile_binding(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                rt.block_on(async move {
                    state.clear_session_binding(session_id.as_str()).await;
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!(
                        "attached proxy does not support session profile binding clear (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/profile"
                            ))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "profile_name": serde_json::Value::Null,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn clear_session_manual_overrides(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                rt.block_on(async move {
                    state
                        .clear_session_manual_overrides(session_id.as_str())
                        .await;
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_session_override_reset {
                    bail!(
                        "attached proxy does not support session manual override reset (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/reset"
                            ))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn set_default_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let service_name = r.service_name;
                let cfg = r.cfg.clone();
                let now = now_ms();
                let effective_default = rt.block_on(async move {
                    let mgr = match service_name {
                        "claude" => &cfg.claude,
                        _ => &cfg.codex,
                    };
                    match profile_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                    {
                        Some(name) => {
                            if mgr.profile(name).is_none() {
                                bail!("profile not found: {name}");
                            }
                            state
                                .set_runtime_default_profile_override(
                                    service_name.to_string(),
                                    name.to_string(),
                                    now,
                                )
                                .await;
                        }
                        None => {
                            state
                                .clear_runtime_default_profile_override(service_name)
                                .await;
                        }
                    }

                    Ok::<_, anyhow::Error>(
                        effective_default_profile_from_cfg_state(
                            state.as_ref(),
                            service_name,
                            cfg.as_ref(),
                        )
                        .await,
                    )
                })?;
                r.default_profile = effective_default.clone();
                r.profiles = list_profiles_from_cfg(
                    r.cfg.as_ref(),
                    r.service_name,
                    effective_default.as_deref(),
                );
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if !att.supports_default_profile_override {
                    bail!("attached proxy does not support runtime default profile switch");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!("{base}/__codex_helper/api/v1/profiles/default"))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                                "profile_name": profile_name,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    #[allow(dead_code)]
    pub fn set_persisted_default_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: Option<String>,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support persisted profile config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
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

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support persisted profile config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
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

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support persisted profile config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_persisted_active_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_persisted_station_config {
                    bail!("attached proxy does not support persisted station config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_persisted_retry_config(
        &mut self,
        rt: &tokio::runtime::Runtime,
        retry: RetryConfig,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_retry_config_api {
                    bail!("attached proxy does not support persisted retry config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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

        self.refresh_current_if_due(rt, Duration::from_secs(0));
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

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_persisted_station_config {
                    bail!("attached proxy does not support persisted station config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
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

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_station_spec_api {
                    bail!(
                        "attached proxy does not support persisted station spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
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

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_station_spec_api {
                    bail!(
                        "attached proxy does not support persisted station spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
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

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_provider_spec_api {
                    bail!(
                        "attached proxy does not support persisted provider spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
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

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_provider_spec_api {
                    bail!(
                        "attached proxy does not support persisted provider spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

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
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn set_runtime_station_meta(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
        enabled: Option<Option<bool>>,
        level: Option<Option<u8>>,
        runtime_state: Option<Option<RuntimeConfigState>>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let service_name = r.service_name;
                let cfg = r.cfg.clone();
                let now = now_ms();
                let stations = rt.block_on(async move {
                    let mgr = match service_name {
                        "claude" => &cfg.claude,
                        _ => &cfg.codex,
                    };
                    if !mgr.contains_station(station_name.as_str()) {
                        bail!("station not found: {station_name}");
                    }

                    if let Some(enabled) = enabled {
                        match enabled {
                            Some(enabled) => {
                                state
                                    .set_station_enabled_override(
                                        service_name,
                                        station_name.clone(),
                                        enabled,
                                        now,
                                    )
                                    .await;
                            }
                            None => {
                                state
                                    .clear_station_enabled_override(
                                        service_name,
                                        station_name.as_str(),
                                    )
                                    .await;
                            }
                        }
                    }

                    if let Some(level) = level {
                        match level {
                            Some(level) => {
                                state
                                    .set_station_level_override(
                                        service_name,
                                        station_name.clone(),
                                        level.clamp(1, 10),
                                        now,
                                    )
                                    .await;
                            }
                            None => {
                                state
                                    .clear_station_level_override(
                                        service_name,
                                        station_name.as_str(),
                                    )
                                    .await;
                            }
                        }
                    }

                    if let Some(runtime_state) = runtime_state {
                        match runtime_state {
                            Some(runtime_state) => {
                                state
                                    .set_station_runtime_state_override(
                                        service_name,
                                        station_name.clone(),
                                        runtime_state,
                                        now,
                                    )
                                    .await;
                            }
                            None => {
                                state
                                    .clear_station_runtime_state_override(
                                        service_name,
                                        station_name.as_str(),
                                    )
                                    .await;
                            }
                        }
                    }

                    Ok::<_, anyhow::Error>(
                        effective_stations_from_cfg_state(
                            state.as_ref(),
                            service_name,
                            cfg.as_ref(),
                        )
                        .await,
                    )
                })?;
                r.stations = stations;
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if !att.supports_station_runtime_override {
                    bail!("attached proxy does not support runtime station meta control");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    let clear_enabled = matches!(enabled, Some(None));
                    let clear_level = matches!(level, Some(None));
                    let clear_runtime_state = matches!(runtime_state, Some(None));
                    let mut body = serde_json::Map::new();
                    body.insert(
                        "station_name".to_string(),
                        serde_json::Value::String(station_name),
                    );
                    body.insert("enabled".to_string(), serde_json::json!(enabled.flatten()));
                    body.insert("level".to_string(), serde_json::json!(level.flatten()));
                    body.insert(
                        "clear_enabled".to_string(),
                        serde_json::json!(clear_enabled),
                    );
                    body.insert("clear_level".to_string(), serde_json::json!(clear_level));
                    body.insert(
                        "runtime_state".to_string(),
                        serde_json::json!(runtime_state.flatten()),
                    );
                    body.insert(
                        "clear_runtime_state".to_string(),
                        serde_json::json!(clear_runtime_state),
                    );
                    send_admin_request(
                        client
                            .post(format!("{base}/__codex_helper/api/v1/stations/runtime"))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::Value::Object(body)),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_station_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match station_name {
                        Some(name) => {
                            state
                                .set_session_station_override(session_id, name, now)
                                .await
                        }
                        None => state.clear_session_station_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!(
                        "attached proxy does not support session station overrides (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/station"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "station_name": station_name,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_service_tier_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        service_tier: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match service_tier {
                        Some(service_tier) => {
                            state
                                .set_session_service_tier_override(session_id, service_tier, now)
                                .await
                        }
                        None => state.clear_session_service_tier_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!(
                        "attached proxy does not support session service tier overrides (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/service-tier"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "service_tier": service_tier,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_global_station_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match station_name {
                        Some(name) => state.set_global_station_override(name, now).await,
                        None => state.clear_global_station_override().await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support global station override (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/global-station"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({ "station_name": station_name })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }
}
