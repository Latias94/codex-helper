mod persisted_mutations;
mod session_mutations;

use std::time::Duration;

use anyhow::bail;

use crate::proxy::local_proxy_base_url;
use crate::state::RuntimeConfigState;

use super::running_refresh::{
    effective_default_profile_from_cfg_state, effective_stations_from_cfg_state,
    list_profiles_from_cfg,
};
use super::{AttachedStatus, ProxyController, ProxyMode, now_ms, send_admin_request};

fn attached_control_base(
    att: &AttachedStatus,
    supported: bool,
    unsupported_message: &'static str,
) -> anyhow::Result<String> {
    if !supported {
        bail!("{unsupported_message}");
    }
    Ok(att.admin_base_url.clone())
}

fn mode_control_base<F>(
    mode: &ProxyMode,
    supported: F,
    unsupported_message: &'static str,
) -> anyhow::Result<String>
where
    F: FnOnce(&AttachedStatus) -> bool,
{
    match mode {
        ProxyMode::Running(r) => Ok(local_proxy_base_url(r.admin_port)),
        ProxyMode::Attached(att) => attached_control_base(att, supported(att), unsupported_message),
        _ => bail!("proxy is not running/attached"),
    }
}

fn refresh_now(controller: &mut ProxyController, rt: &tokio::runtime::Runtime) {
    controller.refresh_current_if_due(rt, Duration::from_secs(0));
}

impl ProxyController {
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
                let base = attached_control_base(
                    att,
                    att.supports_default_profile_override,
                    "attached proxy does not support runtime default profile switch",
                )?;
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
                let base = attached_control_base(
                    att,
                    att.supports_station_runtime_override,
                    "attached proxy does not support runtime station meta control",
                )?;
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
}
