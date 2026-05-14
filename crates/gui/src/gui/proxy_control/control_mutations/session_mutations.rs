use std::time::Duration;

use anyhow::bail;

use crate::config::is_supported_route_graph_config_version;
use crate::proxy::local_proxy_base_url;

use super::super::{ProxyController, ProxyMode, now_ms, send_admin_request};
use super::{attached_child_control_url, attached_control_url};

fn running_supports_route_target_overrides(r: &super::super::RunningProxy) -> bool {
    r.cfg
        .version
        .is_some_and(is_supported_route_graph_config_version)
}

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
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1),
                    "attached proxy does not support session effort overrides (need api v1)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "effort",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    let payload = serde_json::json!({
                        "session_id": session_id,
                        "effort": effort,
                    });
                    send_admin_request(
                        client
                            .post(url)
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
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1),
                    "attached proxy does not support session model overrides (need api v1)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "model",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(800)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                            "model": model,
                        }),
                    ))
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
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1),
                    "attached proxy does not support session profile apply (need api v1)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "profile",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(1200)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                            "profile_name": profile_name,
                        }),
                    ))
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
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1),
                    "attached proxy does not support session profile binding clear (need api v1)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "profile",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(1200)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                            "profile_name": serde_json::Value::Null,
                        }),
                    ))
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
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1) && att.supports_session_override_reset,
                    "attached proxy does not support session manual override reset (need api v1)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "reset",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(1200)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                        }),
                    ))
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
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1),
                    "attached proxy does not support session station overrides (need api v1)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "station",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(800)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                            "station_name": station_name,
                        }),
                    ))
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_route_target_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        route_target: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                if !running_supports_route_target_overrides(r) {
                    bail!("running proxy does not support route target overrides");
                }
                let url = format!(
                    "{}/__codex_helper/api/v1/overrides/session/route",
                    local_proxy_base_url(r.admin_port)
                );
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(800)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                            "target": route_target,
                        }),
                    ))
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            ProxyMode::Attached(att) => {
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1) && att.supports_session_route_target_override,
                    "attached proxy does not support session route target overrides (need api v1 route graph)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "route",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(800)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                            "target": route_target,
                        }),
                    ))
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
                let url = attached_child_control_url(
                    att,
                    att.api_version == Some(1),
                    "attached proxy does not support session service tier overrides (need api v1)",
                    |links| Some(links.session_overrides.as_str()),
                    "/__codex_helper/api/v1/overrides/session",
                    "service-tier",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(client.post(url).timeout(Duration::from_millis(800)).json(
                        &serde_json::json!({
                            "session_id": session_id,
                            "service_tier": service_tier,
                        }),
                    ))
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
                let url = attached_control_url(
                    att,
                    att.api_version == Some(1),
                    "attached proxy does not support global station override (need api v1)",
                    |links| Some(links.global_station_override.as_str()),
                    "/__codex_helper/api/v1/overrides/global-station",
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
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_global_route_target_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        route_target: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                if !running_supports_route_target_overrides(r) {
                    bail!("running proxy does not support global route target overrides");
                }
                let url = format!(
                    "{}/__codex_helper/api/v1/overrides/global-route",
                    local_proxy_base_url(r.admin_port)
                );
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(url)
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({ "target": route_target })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            ProxyMode::Attached(att) => {
                let url = attached_control_url(
                    att,
                    att.api_version == Some(1) && att.supports_global_route_target_override,
                    "attached proxy does not support global route target overrides (need api v1 route graph)",
                    |links| Some(links.global_route_override.as_str()),
                    "/__codex_helper/api/v1/overrides/global-route",
                )?;
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(url)
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({ "target": route_target })),
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
