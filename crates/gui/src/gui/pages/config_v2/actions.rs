use super::*;

#[derive(Default)]
pub(super) struct ConfigV2PendingActions {
    pub(super) set_active: Option<String>,
    pub(super) clear_active: bool,
    pub(super) set_active_remote: Option<Option<String>>,
    pub(super) probe_selected: Option<String>,
    pub(super) health_start: Option<(bool, Vec<String>)>,
    pub(super) health_cancel: Option<(bool, Vec<String>)>,
    pub(super) save_apply: bool,
    pub(super) save_apply_remote: Option<(String, bool, u8)>,
    pub(super) upsert_station_spec_remote: Option<(String, PersistedStationSpec)>,
    pub(super) delete_station_spec_remote: Option<String>,
    pub(super) upsert_provider_spec_remote: Option<(String, PersistedProviderSpec)>,
    pub(super) delete_provider_spec_remote: Option<String>,
    pub(super) profile_upsert_remote: Option<(String, crate::config::ServiceControlProfile)>,
    pub(super) profile_delete_remote: Option<String>,
    pub(super) profile_set_persisted_default_remote: Option<Option<String>>,
}

impl ConfigV2PendingActions {
    pub(super) fn apply(self, ctx: &mut PageCtx<'_>) {
        let Self {
            set_active,
            clear_active,
            set_active_remote,
            probe_selected,
            health_start,
            health_cancel,
            save_apply,
            save_apply_remote,
            upsert_station_spec_remote,
            delete_station_spec_remote,
            upsert_provider_spec_remote,
            delete_provider_spec_remote,
            profile_upsert_remote,
            profile_delete_remote,
            profile_set_persisted_default_remote,
        } = self;

        if let Some((profile_name, profile)) = profile_upsert_remote {
            match ctx
                .proxy
                .upsert_persisted_profile(ctx.rt, profile_name.clone(), profile)
            {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    ctx.view.config.selected_profile_name = Some(profile_name);
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已写入 profile 配置并刷新代理。",
                            "Profile config saved and proxy refreshed.",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("save profile via control plane failed: {e}"));
                }
            }
        }

        if let Some(default_profile_name) = profile_set_persisted_default_remote {
            match ctx
                .proxy
                .set_persisted_default_profile(ctx.rt, default_profile_name.clone())
            {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    *ctx.last_info = Some(
                        match default_profile_name {
                            Some(_) => pick(
                                ctx.lang,
                                "已更新配置默认 profile。",
                                "Configured default profile updated.",
                            ),
                            None => pick(
                                ctx.lang,
                                "已清除配置默认 profile。",
                                "Configured default profile cleared.",
                            ),
                        }
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!(
                        "set persisted default profile via control plane failed: {e}"
                    ));
                }
            }
        }

        if let Some(profile_name) = profile_delete_remote {
            match ctx.proxy.delete_persisted_profile(ctx.rt, profile_name) {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    ctx.view.config.selected_profile_name = None;
                    ctx.view.config.profile_editor = ConfigProfileEditorState::default();
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已删除 profile 并刷新代理。",
                            "Profile deleted and proxy refreshed.",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!(
                        "delete persisted profile via control plane failed: {e}"
                    ));
                }
            }
        }

        if let Some(station_name) = set_active_remote {
            match ctx
                .proxy
                .set_persisted_active_station(ctx.rt, station_name.clone())
            {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    *ctx.last_info = Some(
                        match station_name {
                            Some(_) => pick(
                                ctx.lang,
                                "已更新配置 active_station 并刷新代理。",
                                "Configured active_station updated and proxy refreshed.",
                            ),
                            None => pick(
                                ctx.lang,
                                "已清除配置 active_station 并刷新代理。",
                                "Configured active_station cleared and proxy refreshed.",
                            ),
                        }
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!(
                        "set persisted active station via control plane failed: {e}"
                    ));
                }
            }
        }

        if let Some((station_name, enabled, level)) = save_apply_remote {
            match ctx
                .proxy
                .update_persisted_station(ctx.rt, station_name, enabled, level)
            {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已保存 station 配置并刷新代理。",
                            "Station config saved and proxy refreshed.",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("save station via control plane failed: {e}"));
                }
            }
        }

        if let Some((station_name, station_spec)) = upsert_station_spec_remote {
            match ctx.proxy.upsert_persisted_station_spec(
                ctx.rt,
                station_name.clone(),
                station_spec,
            ) {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    ctx.view.config.selected_name = Some(station_name);
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已写入 station 结构并刷新代理。",
                            "Station structure saved and proxy refreshed.",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!(
                        "save station structure via control plane failed: {e}"
                    ));
                }
            }
        }

        if let Some(station_name) = delete_station_spec_remote {
            match ctx
                .proxy
                .delete_persisted_station_spec(ctx.rt, station_name)
            {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    ctx.view.config.selected_name = None;
                    ctx.view.config.station_editor = ConfigStationEditorState::default();
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已删除 station 并刷新代理。",
                            "Station deleted and proxy refreshed.",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!(
                        "delete station structure via control plane failed: {e}"
                    ));
                }
            }
        }

        if let Some((provider_name, provider_spec)) = upsert_provider_spec_remote {
            match ctx.proxy.upsert_persisted_provider_spec(
                ctx.rt,
                provider_name.clone(),
                provider_spec,
            ) {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    ctx.view.config.selected_provider_name = Some(provider_name);
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已写入 provider 结构并刷新代理。",
                            "Provider structure saved and proxy refreshed.",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!(
                        "save provider structure via control plane failed: {e}"
                    ));
                }
            }
        }

        if let Some(provider_name) = delete_provider_spec_remote {
            match ctx
                .proxy
                .delete_persisted_provider_spec(ctx.rt, provider_name)
            {
                Ok(()) => {
                    refresh_proxy_and_editor(ctx);
                    ctx.view.config.selected_provider_name = None;
                    ctx.view.config.provider_editor = ConfigProviderEditorState::default();
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已删除 provider 并刷新代理。",
                            "Provider deleted and proxy refreshed.",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!(
                        "delete provider structure via control plane failed: {e}"
                    ));
                }
            }
        }

        if let Some(name) = set_active {
            set_local_active_group(ctx, Some(name));
            *ctx.last_info =
                Some(pick(ctx.lang, "已设置 active_station", "active_station set").to_string());
        }

        if clear_active {
            set_local_active_group(ctx, None);
            *ctx.last_info =
                Some(pick(ctx.lang, "已清除 active_station", "active_station cleared").to_string());
        }

        if let Some((all, names)) = health_start {
            if let Err(e) = ctx.proxy.start_health_checks(ctx.rt, all, names) {
                *ctx.last_error = Some(format!("health check start failed: {e}"));
            } else {
                *ctx.last_info =
                    Some(pick(ctx.lang, "已开始健康检查", "Health check started").to_string());
            }
        }

        if let Some(name) = probe_selected {
            if let Err(e) = ctx.proxy.probe_station(ctx.rt, name) {
                *ctx.last_error = Some(format!("station probe failed: {e}"));
            } else {
                *ctx.last_info = Some(pick(ctx.lang, "已开始探测", "Probe started").to_string());
            }
        }

        if let Some((all, names)) = health_cancel {
            if let Err(e) = ctx.proxy.cancel_health_checks(ctx.rt, all, names) {
                *ctx.last_error = Some(format!("health check cancel failed: {e}"));
            } else {
                *ctx.last_info = Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
            }
        }

        if save_apply {
            save_apply_local(ctx);
        }
    }
}

fn refresh_proxy_and_editor(ctx: &mut PageCtx<'_>) {
    ctx.proxy
        .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
    refresh_config_editor_from_disk_if_running(ctx);
}

fn set_local_active_group(ctx: &mut PageCtx<'_>, active_group: Option<String>) {
    let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
        return;
    };
    let view = match ctx.view.config.service {
        crate::config::ServiceKind::Claude => &mut cfg.claude,
        crate::config::ServiceKind::Codex => &mut cfg.codex,
    };
    view.active_group = active_group;
}

fn save_apply_local(ctx: &mut PageCtx<'_>) {
    let save_res = {
        let cfg = ctx.view.config.working.as_ref().expect("checked above");
        save_proxy_config_document(ctx.rt, cfg)
    };
    match save_res {
        Ok(()) => {
            let new_path = crate::config::config_file_path();
            if let Ok(t) = std::fs::read_to_string(&new_path) {
                *ctx.proxy_config_text = t;
            }
            if let Ok(t) = std::fs::read_to_string(&new_path)
                && let Ok(parsed) = parse_proxy_config_document(&t)
            {
                ctx.view.config.working = Some(parsed);
            }

            if matches!(
                ctx.proxy.kind(),
                ProxyModeKind::Running | ProxyModeKind::Attached
            ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
            {
                *ctx.last_error = Some(format!("reload runtime failed: {e}"));
            }

            *ctx.last_info = Some(pick(ctx.lang, "已保存", "Saved").to_string());
            *ctx.last_error = None;
        }
        Err(e) => {
            *ctx.last_error = Some(format!("save failed: {e}"));
        }
    }
}
