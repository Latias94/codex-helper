use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(6.0);
    ui.label(pick(
        ctx.lang,
        "当前文件是 v2 station/provider 布局。表单视图现在支持 station/provider/profile 的常用结构管理；provider tags、supported_models、model_mapping 等高级字段仍建议用“原始”视图。",
        "This file uses the v2 station/provider schema. Form view now covers common station/provider/profile structure management; use Raw view for advanced provider tags, supported_models, and model_mapping edits.",
    ));

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "服务", "Service"));
        let mut svc = ctx.view.config.service;
        egui::ComboBox::from_id_salt("config_form_v2_service")
            .selected_text(match svc {
                crate::config::ServiceKind::Codex => "codex",
                crate::config::ServiceKind::Claude => "claude",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Codex, "codex");
                ui.selectable_value(&mut svc, crate::config::ServiceKind::Claude, "claude");
            });
        ctx.view.config.service = svc;
    });

    let (
        schema_version,
        active_name,
        active_fallback,
        default_profile,
        station_names,
        profile_names,
    ) = {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_ref() else {
            return;
        };
        let runtime = crate::config::compile_v2_to_runtime(cfg).ok();
        let (view, runtime_mgr) = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => {
                (&cfg.claude, runtime.as_ref().map(|r| &r.claude))
            }
            crate::config::ServiceKind::Codex => (&cfg.codex, runtime.as_ref().map(|r| &r.codex)),
        };
        let mut names = view.groups.keys().cloned().collect::<Vec<_>>();
        names.sort_by(|a, b| {
            let la = view.groups.get(a).map(|c| c.level).unwrap_or(1);
            let lb = view.groups.get(b).map(|c| c.level).unwrap_or(1);
            la.cmp(&lb).then_with(|| a.cmp(b))
        });
        let profiles = view.profiles.keys().cloned().collect::<Vec<_>>();
        (
            cfg.version,
            view.active_group.clone(),
            runtime_mgr.and_then(|mgr| mgr.active_config().map(|cfg| cfg.name.clone())),
            view.default_profile.clone(),
            names,
            profiles,
        )
    };

    let selected_service = match ctx.view.config.service {
        crate::config::ServiceKind::Claude => "claude",
        crate::config::ServiceKind::Codex => "codex",
    };
    let control_plane_snapshot = ctx.proxy.snapshot().filter(|snapshot| {
        snapshot.supports_v1 && snapshot.service_name.as_deref() == Some(selected_service)
    });
    let station_control_plane_snapshot = control_plane_snapshot
        .clone()
        .filter(|snapshot| snapshot.supports_persisted_station_config);
    let station_control_plane_catalog = station_control_plane_snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .stations
                .iter()
                .cloned()
                .map(|config| (config.name.clone(), config))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let station_control_plane_enabled = station_control_plane_snapshot.is_some();
    let station_control_plane_configured_active = station_control_plane_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.configured_active_station.clone());
    let station_control_plane_effective_active = station_control_plane_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.effective_active_station.clone());
    let station_default_profile = if station_control_plane_enabled {
        station_control_plane_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.configured_default_profile.clone())
            .or_else(|| {
                station_control_plane_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.default_profile.clone())
            })
    } else {
        default_profile.clone()
    };
    let attached_station_specs = ctx
        .proxy
        .attached()
        .filter(|att| {
            att.service_name.as_deref() == Some(selected_service) && att.supports_station_spec_api
        })
        .map(|att| {
            (
                att.persisted_stations.clone(),
                att.persisted_station_providers.clone(),
            )
        });
    let station_structure_control_plane_enabled = attached_station_specs.is_some();
    let station_structure_edit_enabled = station_structure_control_plane_enabled
        || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached);
    let attached_provider_specs = ctx
        .proxy
        .attached()
        .filter(|att| {
            att.service_name.as_deref() == Some(selected_service) && att.supports_provider_spec_api
        })
        .map(|att| att.persisted_providers.clone());
    let provider_structure_control_plane_enabled = attached_provider_specs.is_some();
    let provider_structure_edit_enabled = provider_structure_control_plane_enabled
        || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached);
    let station_display_names = if let Some((stations, _)) = attached_station_specs.as_ref() {
        let mut names = stations.values().cloned().collect::<Vec<_>>();
        names.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
        names
            .into_iter()
            .map(|station| station.name)
            .collect::<Vec<_>>()
    } else if let Some(snapshot) = station_control_plane_snapshot.as_ref() {
        let mut names = snapshot.stations.clone();
        names.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
        names
            .into_iter()
            .map(|config| config.name)
            .collect::<Vec<_>>()
    } else {
        station_names.clone()
    };

    if ctx
        .view
        .config
        .selected_name
        .as_ref()
        .is_none_or(|n| !station_display_names.iter().any(|x| x == n))
    {
        ctx.view.config.selected_name = station_display_names.first().cloned();
    }
    let selected_name = ctx.view.config.selected_name.clone();
    let selected_station_name = selected_name.clone().unwrap_or_default();
    let (runtime_service, supports_v1, cfg_health, hc_status): (
        Option<String>,
        bool,
        Option<StationHealth>,
        Option<HealthCheckStatus>,
    ) = match ctx.proxy.kind() {
        ProxyModeKind::Running => {
            if let Some(r) = ctx.proxy.running() {
                let state = r.state.clone();
                let (health, checks) = ctx.rt.block_on(async {
                    tokio::join!(
                        state.get_station_health(r.service_name),
                        state.list_health_checks(r.service_name)
                    )
                });
                (
                    Some(r.service_name.to_string()),
                    true,
                    health.get(&selected_station_name).cloned(),
                    checks.get(&selected_station_name).cloned(),
                )
            } else {
                (None, false, None, None)
            }
        }
        ProxyModeKind::Attached => {
            if let Some(att) = ctx.proxy.attached() {
                (
                    att.service_name.clone(),
                    att.api_version == Some(1),
                    att.station_health.get(&selected_station_name).cloned(),
                    att.health_checks.get(&selected_station_name).cloned(),
                )
            } else {
                (None, false, None, None)
            }
        }
        _ => (None, false, None, None),
    };

    let mut action_set_active: Option<String> = None;
    let mut action_clear_active = false;
    let mut action_set_active_remote: Option<Option<String>> = None;
    let mut action_probe_selected: Option<String> = None;
    let mut action_health_start: Option<(bool, Vec<String>)> = None;
    let mut action_health_cancel: Option<(bool, Vec<String>)> = None;
    let mut action_save_apply = false;
    let mut action_save_apply_remote: Option<(String, bool, u8)> = None;
    let mut action_upsert_station_spec_remote: Option<(String, PersistedStationSpec)> = None;
    let mut action_delete_station_spec_remote: Option<String> = None;
    let mut action_upsert_provider_spec_remote: Option<(String, PersistedProviderSpec)> = None;
    let mut action_delete_provider_spec_remote: Option<String> = None;
    let mut station_editor_name = ctx.view.config.station_editor.station_name.clone();
    let mut station_editor_alias = ctx.view.config.station_editor.alias.clone();
    let mut station_editor_enabled = ctx.view.config.station_editor.enabled;
    let mut station_editor_level = ctx.view.config.station_editor.level.max(1);
    let mut station_editor_members = ctx.view.config.station_editor.members.clone();
    let mut new_station_name = ctx.view.config.station_editor.new_station_name.clone();
    let mut selected_provider_name = ctx.view.config.selected_provider_name.clone();
    let mut provider_editor_name = ctx.view.config.provider_editor.provider_name.clone();
    let mut provider_editor_alias = ctx.view.config.provider_editor.alias.clone();
    let mut provider_editor_enabled = ctx.view.config.provider_editor.enabled;
    let mut provider_editor_auth_token_env = ctx.view.config.provider_editor.auth_token_env.clone();
    let mut provider_editor_api_key_env = ctx.view.config.provider_editor.api_key_env.clone();
    let mut provider_editor_endpoints = ctx.view.config.provider_editor.endpoints.clone();
    let mut new_provider_name = ctx.view.config.provider_editor.new_provider_name.clone();
    if station_structure_control_plane_enabled {
        let selected_station = selected_name.as_deref().and_then(|name| {
            attached_station_specs
                .as_ref()
                .and_then(|specs| specs.0.get(name))
        });
        if station_editor_name.as_deref() != selected_name.as_deref() {
            station_editor_name = selected_name.clone();
            station_editor_alias = selected_station
                .and_then(|station| station.alias.clone())
                .unwrap_or_default();
            station_editor_enabled = selected_station
                .map(|station| station.enabled)
                .unwrap_or(true);
            station_editor_level = selected_station
                .map(|station| station.level)
                .unwrap_or(1)
                .clamp(1, 10);
            station_editor_members = selected_station
                .map(|station| {
                    station
                        .members
                        .iter()
                        .map(config_station_member_editor_from_member)
                        .collect()
                })
                .unwrap_or_default();
        }
    } else if station_control_plane_enabled {
        let selected_station = selected_name
            .as_deref()
            .and_then(|name| station_control_plane_catalog.get(name));
        if station_editor_name.as_deref() != selected_name.as_deref() {
            station_editor_name = selected_name.clone();
            station_editor_alias = String::new();
            station_editor_enabled = selected_station
                .map(|station| station.enabled)
                .unwrap_or(false);
            station_editor_level = selected_station
                .map(|station| station.level)
                .unwrap_or(1)
                .clamp(1, 10);
            station_editor_members.clear();
        }
    }
    let profile_control_plane_snapshot = control_plane_snapshot;
    let profile_control_plane_catalog = profile_control_plane_snapshot
        .as_ref()
        .map(|snapshot| {
            snapshot
                .profiles
                .iter()
                .map(|profile| (profile.name.clone(), service_profile_from_option(profile)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let profile_control_plane_default = profile_control_plane_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.configured_default_profile.clone())
        .or_else(|| {
            profile_control_plane_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.default_profile.clone())
        });
    let profile_control_plane_station_names = profile_control_plane_snapshot
        .as_ref()
        .map(|snapshot| {
            let mut names = snapshot
                .stations
                .iter()
                .map(|config| config.name.clone())
                .collect::<Vec<_>>();
            names.sort();
            names.dedup();
            names
        })
        .unwrap_or_else(|| station_names.clone());
    let profile_control_plane_enabled = profile_control_plane_snapshot.is_some();
    let mut selected_profile_name = ctx.view.config.selected_profile_name.clone();
    if profile_control_plane_enabled {
        if selected_profile_name
            .as_ref()
            .is_none_or(|name| !profile_control_plane_catalog.contains_key(name))
        {
            selected_profile_name = profile_control_plane_default
                .clone()
                .or_else(|| profile_control_plane_catalog.keys().next().cloned());
        }
    } else if selected_profile_name
        .as_ref()
        .is_none_or(|name| !profile_names.iter().any(|item| item == name))
    {
        selected_profile_name = default_profile
            .clone()
            .or_else(|| profile_names.first().cloned());
    }
    let mut new_profile_name = ctx.view.config.new_profile_name.clone();
    let mut profile_editor_name = ctx.view.config.profile_editor.profile_name.clone();
    let mut profile_editor_extends = ctx.view.config.profile_editor.extends.clone();
    let mut profile_editor_station = ctx.view.config.profile_editor.station.clone();
    let mut profile_editor_model = ctx.view.config.profile_editor.model.clone();
    let mut profile_editor_reasoning_effort =
        ctx.view.config.profile_editor.reasoning_effort.clone();
    let mut profile_editor_service_tier = ctx.view.config.profile_editor.service_tier.clone();
    let mut profile_info: Option<String> = None;
    let mut profile_error: Option<String> = None;
    let mut action_profile_upsert_remote: Option<(String, crate::config::ServiceControlProfile)> =
        None;
    let mut action_profile_delete_remote: Option<String> = None;
    let mut action_profile_set_persisted_default_remote: Option<Option<String>> = None;

    if profile_control_plane_enabled {
        let selected_profile = selected_profile_name
            .as_deref()
            .and_then(|name| profile_control_plane_catalog.get(name));
        if profile_editor_name.as_deref() != selected_profile_name.as_deref() {
            profile_editor_name = selected_profile_name.clone();
            profile_editor_extends = selected_profile.and_then(|profile| profile.extends.clone());
            profile_editor_station = selected_profile.and_then(|profile| profile.station.clone());
            profile_editor_model = selected_profile
                .and_then(|profile| profile.model.clone())
                .unwrap_or_default();
            profile_editor_reasoning_effort = selected_profile
                .and_then(|profile| profile.reasoning_effort.clone())
                .unwrap_or_default();
            profile_editor_service_tier = selected_profile
                .and_then(|profile| profile.service_tier.clone())
                .unwrap_or_default();
        }
    }

    {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
            return;
        };
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        let provider_catalog = view.providers.clone();
        let local_provider_catalog = crate::config::build_persisted_provider_catalog(view);
        let local_provider_spec_catalog = local_provider_catalog
            .providers
            .iter()
            .cloned()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();
        let local_station_catalog = crate::config::build_persisted_station_catalog(view);
        let local_station_spec_catalog = local_station_catalog
            .stations
            .iter()
            .cloned()
            .map(|station| (station.name.clone(), station))
            .collect::<BTreeMap<_, _>>();
        let local_provider_ref_catalog = local_station_catalog
            .providers
            .iter()
            .cloned()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();
        let attached_mode = matches!(ctx.proxy.kind(), ProxyModeKind::Attached);
        let preview_station_specs = if station_structure_control_plane_enabled {
            attached_station_specs.as_ref().map(|specs| &specs.0)
        } else if attached_mode {
            None
        } else {
            Some(&local_station_spec_catalog)
        };
        let preview_provider_catalog = if station_structure_control_plane_enabled {
            attached_station_specs.as_ref().map(|specs| &specs.1)
        } else if attached_mode {
            None
        } else {
            Some(&local_provider_ref_catalog)
        };
        let preview_runtime_station_catalog = station_control_plane_snapshot
            .as_ref()
            .map(|_| &station_control_plane_catalog);
        if !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            && station_editor_name.as_deref() != selected_name.as_deref()
        {
            let selected_station = selected_name
                .as_deref()
                .and_then(|name| local_station_spec_catalog.get(name));
            station_editor_name = selected_name.clone();
            station_editor_alias = selected_station
                .and_then(|station| station.alias.clone())
                .unwrap_or_default();
            station_editor_enabled = selected_station
                .map(|station| station.enabled)
                .unwrap_or(true);
            station_editor_level = selected_station
                .map(|station| station.level)
                .unwrap_or(1)
                .clamp(1, 10);
            station_editor_members = selected_station
                .map(|station| {
                    station
                        .members
                        .iter()
                        .map(config_station_member_editor_from_member)
                        .collect()
                })
                .unwrap_or_default();
        }
        let mut provider_display_names =
            if let Some(provider_specs) = attached_provider_specs.as_ref() {
                provider_specs.keys().cloned().collect::<Vec<_>>()
            } else {
                local_provider_spec_catalog
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            };
        provider_display_names.sort();
        if selected_provider_name
            .as_ref()
            .is_none_or(|name| !provider_display_names.iter().any(|item| item == name))
        {
            selected_provider_name = provider_display_names.first().cloned();
        }
        if provider_structure_control_plane_enabled {
            let selected_provider = selected_provider_name.as_deref().and_then(|name| {
                attached_provider_specs
                    .as_ref()
                    .and_then(|specs| specs.get(name))
            });
            if provider_editor_name.as_deref() != selected_provider_name.as_deref() {
                provider_editor_name = selected_provider_name.clone();
                provider_editor_alias = selected_provider
                    .and_then(|provider| provider.alias.clone())
                    .unwrap_or_default();
                provider_editor_enabled = selected_provider
                    .map(|provider| provider.enabled)
                    .unwrap_or(true);
                provider_editor_auth_token_env = selected_provider
                    .and_then(|provider| provider.auth_token_env.clone())
                    .unwrap_or_default();
                provider_editor_api_key_env = selected_provider
                    .and_then(|provider| provider.api_key_env.clone())
                    .unwrap_or_default();
                provider_editor_endpoints = selected_provider
                    .map(|provider| {
                        provider
                            .endpoints
                            .iter()
                            .map(config_provider_endpoint_editor_from_spec)
                            .collect()
                    })
                    .unwrap_or_default();
            }
        } else if !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            && provider_editor_name.as_deref() != selected_provider_name.as_deref()
        {
            let selected_provider = selected_provider_name
                .as_deref()
                .and_then(|name| local_provider_spec_catalog.get(name));
            provider_editor_name = selected_provider_name.clone();
            provider_editor_alias = selected_provider
                .and_then(|provider| provider.alias.clone())
                .unwrap_or_default();
            provider_editor_enabled = selected_provider
                .map(|provider| provider.enabled)
                .unwrap_or(true);
            provider_editor_auth_token_env = selected_provider
                .and_then(|provider| provider.auth_token_env.clone())
                .unwrap_or_default();
            provider_editor_api_key_env = selected_provider
                .and_then(|provider| provider.api_key_env.clone())
                .unwrap_or_default();
            provider_editor_endpoints = selected_provider
                .map(|provider| {
                    provider
                        .endpoints
                        .iter()
                        .map(config_provider_endpoint_editor_from_spec)
                        .collect()
                })
                .unwrap_or_default();
        }
        let profile_catalog = view.profiles.clone();
        let configured_active_name = if station_control_plane_enabled {
            station_control_plane_configured_active.clone()
        } else {
            active_name.clone()
        };
        let effective_active_name = if station_control_plane_enabled {
            station_control_plane_effective_active.clone()
        } else if active_name.is_some() {
            active_name.clone()
        } else {
            active_fallback.clone()
        };

        ui.columns(2, |cols| {
            cols[0].heading(pick(ctx.lang, "站点列表", "Stations"));
            cols[0].add_space(4.0);
            cols[0].horizontal(|ui| {
                ui.label(pick(ctx.lang, "新建 station", "New station"));
                ui.add_sized(
                    [180.0, 22.0],
                    egui::TextEdit::singleline(&mut new_station_name).hint_text(pick(
                        ctx.lang,
                        "例如 primary / backup",
                        "e.g. primary / backup",
                    )),
                );
                if ui
                    .add_enabled(
                        station_structure_edit_enabled,
                        egui::Button::new(pick(ctx.lang, "新增", "Add")),
                    )
                    .clicked()
                {
                    let name = new_station_name.trim();
                    if name.is_empty() {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "station 名称不能为空。",
                                "Station name cannot be empty.",
                            )
                            .to_string(),
                        );
                    } else if station_structure_control_plane_enabled {
                        if attached_station_specs
                            .as_ref()
                            .is_some_and(|specs| specs.0.contains_key(name))
                        {
                            *ctx.last_error = Some(
                                pick(
                                    ctx.lang,
                                    "station 名称已存在。",
                                    "Station name already exists.",
                                )
                                .to_string(),
                            );
                        } else {
                            action_upsert_station_spec_remote = Some((
                                name.to_string(),
                                PersistedStationSpec {
                                    name: name.to_string(),
                                    alias: None,
                                    enabled: true,
                                    level: 1,
                                    members: Vec::new(),
                                },
                            ));
                            ctx.view.config.selected_name = Some(name.to_string());
                            station_editor_name = Some(name.to_string());
                            station_editor_alias.clear();
                            station_editor_enabled = true;
                            station_editor_level = 1;
                            station_editor_members.clear();
                            new_station_name.clear();
                        }
                    } else if view.groups.contains_key(name) {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "station 名称已存在。",
                                "Station name already exists.",
                            )
                            .to_string(),
                        );
                    } else {
                        view.groups.insert(name.to_string(), GroupConfigV2::default());
                        ctx.view.config.selected_name = Some(name.to_string());
                        new_station_name.clear();
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已新增 station（待保存）。",
                                "Station added (save pending).",
                            )
                            .to_string(),
                        );
                    }
                }
            });
            if !station_structure_edit_enabled {
                cols[0].small(pick(
                    ctx.lang,
                    "当前附着目标还没有暴露 station 结构 API，因此这里暂时不能新增/删除 station。",
                    "This attached target does not expose station structure APIs yet, so station create/delete is unavailable here.",
                ));
            }
            cols[0].add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("config_v2_stations_scroll")
                .max_height(520.0)
                .show(&mut cols[0], |ui| {
                    if station_display_names.is_empty() {
                        ui.label(pick(
                            ctx.lang,
                            "当前没有 station。可以先新增一个空 station，再补 member/provider 引用。",
                            "No stations yet. Add an empty station first, then fill member/provider refs.",
                        ));
                    }
                    for name in station_display_names.iter() {
                        let is_active = configured_active_name.as_deref() == Some(name.as_str());
                        let is_fallback_active = configured_active_name.is_none()
                            && effective_active_name.as_deref() == Some(name.as_str());
                        let is_selected = selected_name.as_deref() == Some(name.as_str());

                        let mut label = if station_control_plane_enabled {
                            let station = station_control_plane_catalog.get(name);
                            let (enabled, level, alias) = station
                                .map(|station| {
                                    (
                                        station.enabled,
                                        station.level.clamp(1, 10),
                                        station.alias.as_deref().unwrap_or(""),
                                    )
                                })
                                .unwrap_or((false, 1, ""));
                            let mut label = format!("L{level} {name}");
                            if !alias.trim().is_empty() {
                                label.push_str(&format!(" ({alias})"));
                            }
                            if !enabled {
                                label.push_str("  [off]");
                            }
                            label
                        } else {
                            let station = view.groups.get(name);
                            let (enabled, level, alias, members, endpoint_refs) = station
                                .map(|station| {
                                    let endpoint_refs = station
                                        .members
                                        .iter()
                                        .map(|member| {
                                            provider_catalog
                                                .get(&member.provider)
                                                .map(|provider| {
                                                    if member.endpoint_names.is_empty() {
                                                        provider.endpoints.len()
                                                    } else {
                                                        member.endpoint_names.len()
                                                    }
                                                })
                                                .unwrap_or(0)
                                        })
                                        .sum::<usize>();
                                    (
                                        station.enabled,
                                        station.level.clamp(1, 10),
                                        station.alias.as_deref().unwrap_or(""),
                                        station.members.len(),
                                        endpoint_refs,
                                    )
                                })
                                .unwrap_or((false, 1, "", 0, 0));

                            let mut label = format!("L{level} {name}");
                            if !alias.trim().is_empty() {
                                label.push_str(&format!(" ({alias})"));
                            }
                            label.push_str(&format!("  members={members} refs={endpoint_refs}"));
                            if !enabled {
                                label.push_str("  [off]");
                            }
                            label
                        };
                        if station_control_plane_enabled
                            && let Some(station) = station_control_plane_catalog.get(name)
                        {
                            if let Some(override_enabled) = station.runtime_enabled_override {
                                label.push_str(&format!("  rt_enabled={override_enabled}"));
                            }
                            if let Some(override_level) = station.runtime_level_override {
                                label.push_str(&format!("  rt_level={override_level}"));
                            }
                        }
                        if is_active {
                            label = format!("★ {label}");
                        } else if is_fallback_active {
                            label = format!("◇ {label}");
                        }

                        if ui.selectable_label(is_selected, label).clicked() {
                            ctx.view.config.selected_name = Some(name.clone());
                        }
                    }
                });

            cols[1].heading(pick(ctx.lang, "站点详情", "Station Details"));
            cols[1].add_space(4.0);

            let Some(name) = ctx.view.config.selected_name.clone() else {
                cols[1].label(pick(ctx.lang, "未选择站点。", "No station selected."));
                return;
            };

            let active_label = if configured_active_name.as_deref() == Some(name.as_str()) {
                pick(ctx.lang, "是", "yes")
            } else {
                pick(ctx.lang, "否", "no")
            };
            let effective_label = effective_active_name
                .clone()
                .unwrap_or_else(|| pick(ctx.lang, "(无)", "(none)").to_string());

            cols[1].label(format!("schema: v{schema_version}"));
            cols[1].label(format!("active_station: {active_label}"));
            cols[1].label(format!(
                "{}: {effective_label}",
                pick(ctx.lang, "生效站点", "Effective station")
            ));
            cols[1].label(format!(
                "default_profile: {}",
                station_default_profile.as_deref().unwrap_or("-")
            ));
            cols[1].add_space(6.0);

            if station_structure_control_plane_enabled
                || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            {
                let (station_snapshot, provider_ref_catalog) = if station_structure_control_plane_enabled {
                    let Some((station_specs, provider_specs)) = attached_station_specs.as_ref() else {
                        cols[1].label(pick(
                            ctx.lang,
                            "远端 station 结构视图不可用。",
                            "Remote station structure view is unavailable.",
                        ));
                        return;
                    };
                    let Some(station_snapshot) = station_specs.get(&name).cloned() else {
                        cols[1].label(pick(
                            ctx.lang,
                            "远端 station 不存在（可能已被删除）。",
                            "Remote station missing.",
                        ));
                        return;
                    };
                    (station_snapshot, provider_specs)
                } else {
                    let Some(station_snapshot) = local_station_spec_catalog.get(&name).cloned() else {
                        cols[1].label(pick(
                            ctx.lang,
                            "站点不存在（可能已被删除）。",
                            "Station missing.",
                        ));
                        return;
                    };
                    (station_snapshot, &local_provider_ref_catalog)
                };

                let referencing_profiles = profile_catalog
                    .iter()
                    .filter_map(|(profile_name, profile)| {
                        (profile.station.as_deref() == Some(name.as_str()))
                            .then_some(profile_name.clone())
                    })
                    .collect::<Vec<_>>();

                cols[1].colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    if station_structure_control_plane_enabled {
                        pick(
                            ctx.lang,
                            "当前通过附着代理暴露的 station 结构 API 直接管理远端配置；provider 密钥仍不会通过这里暴露。",
                            "This view manages the attached proxy through its station structure API directly; provider secrets are still not exposed here.",
                        )
                    } else {
                        pick(
                            ctx.lang,
                            "这里编辑的是本机 v2 station/provider 结构；保存后会重载当前代理。",
                            "This edits the local v2 station/provider structure; saving will reload the current proxy.",
                        )
                    },
                );
                cols[1].add_space(6.0);
                cols[1].label(format!("name: {}", name));
                cols[1].label(format!("members: {}", station_snapshot.members.len()));
                cols[1].label(format!(
                    "profiles: {}",
                    if referencing_profiles.is_empty() {
                        "-".to_string()
                    } else {
                        referencing_profiles.join(", ")
                    }
                ));

                cols[1].horizontal(|ui| {
                    ui.label("alias");
                    ui.add_sized(
                        [220.0, 22.0],
                        egui::TextEdit::singleline(&mut station_editor_alias),
                    );
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        station_editor_alias.clear();
                    }
                });

                cols[1].horizontal(|ui| {
                    ui.checkbox(&mut station_editor_enabled, pick(ctx.lang, "启用", "Enabled"));
                    ui.label(pick(ctx.lang, "等级", "Level"));
                    ui.add(egui::DragValue::new(&mut station_editor_level).range(1..=10));
                });

                cols[1].add_space(8.0);
                cols[1].separator();
                cols[1].label(pick(ctx.lang, "成员引用", "Members"));
                render_config_station_member_editor(
                    &mut cols[1],
                    ctx.lang,
                    selected_service,
                    provider_ref_catalog,
                    &mut station_editor_members,
                );

                cols[1].add_space(8.0);
                cols[1].separator();
                cols[1].label(pick(ctx.lang, "可用 Provider", "Available Providers"));
                render_config_station_provider_summary(
                    &mut cols[1],
                    ctx.lang,
                    provider_ref_catalog,
                    &station_editor_members,
                );

                cols[1].add_space(8.0);
                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "设为 active_station", "Set active_station"))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(Some(name.clone()));
                        } else {
                            action_set_active = Some(name.clone());
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            "清除 active_station",
                            "Clear active_station",
                        ))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(None);
                        } else {
                            action_clear_active = true;
                        }
                    }

                    if ui.button(pick(ctx.lang, "删除 station", "Delete station")).clicked() {
                        if !referencing_profiles.is_empty() {
                            *ctx.last_error = Some(format!(
                                "{}: {}",
                                pick(
                                    ctx.lang,
                                    "仍有 profile 引用了该 station，不能删除",
                                    "Profiles still reference this station; delete is blocked",
                                ),
                                referencing_profiles.join(", ")
                            ));
                        } else if station_structure_control_plane_enabled {
                            action_delete_station_spec_remote = Some(name.clone());
                        } else {
                            view.groups.remove(name.as_str());
                            if view.active_group.as_deref() == Some(name.as_str()) {
                                view.active_group = None;
                            }
                            ctx.view.config.selected_name = view.groups.keys().next().cloned();
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已删除 station（待保存）。",
                                    "Station deleted (save pending).",
                                )
                                .to_string(),
                            );
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            if station_structure_control_plane_enabled {
                                "保存到当前代理"
                            } else {
                                "保存并应用"
                            },
                            if station_structure_control_plane_enabled {
                                "Save to current proxy"
                            } else {
                                "Save & apply"
                            },
                        ))
                        .clicked()
                    {
                        match build_station_spec_from_config_editor(
                            name.as_str(),
                            station_editor_alias.as_str(),
                            station_editor_enabled,
                            station_editor_level,
                            &station_editor_members,
                        ) {
                            Ok(station_spec) => {
                                if station_structure_control_plane_enabled {
                                    action_upsert_station_spec_remote =
                                        Some((name.clone(), station_spec));
                                } else {
                                    view.groups.insert(
                                        name.clone(),
                                        GroupConfigV2 {
                                            alias: station_spec.alias.clone(),
                                            enabled: station_spec.enabled,
                                            level: station_spec.level,
                                            members: station_spec.members.clone(),
                                        },
                                    );
                                    action_save_apply = true;
                                }
                            }
                            Err(e) => {
                                *ctx.last_error = Some(e);
                            }
                        }
                    }
                });
            } else {
                let Some(station_snapshot) = station_control_plane_catalog.get(&name).cloned() else {
                    cols[1].label(pick(
                        ctx.lang,
                        "远端 station 不存在（可能已被删除）。",
                        "Remote station missing.",
                    ));
                    return;
                };
                cols[1].colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(
                        ctx.lang,
                        "当前 common station 字段直接管理运行中的代理；provider/member 结构请回到原始视图或本机文件查看。",
                        "Common station fields below manage the live proxy directly; use Raw view or the local file for provider/member structure.",
                    ),
                );
                cols[1].add_space(6.0);
                cols[1].label(format!("name: {}", name));
                cols[1].label(format!(
                    "alias: {}",
                    station_snapshot.alias.as_deref().unwrap_or("-")
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "配置启用", "Configured enabled"),
                    station_snapshot.configured_enabled
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "配置等级", "Configured level"),
                    station_snapshot.configured_level
                ));
                cols[1].label(format!(
                    "{}: {:?}",
                    pick(ctx.lang, "运行状态", "Runtime state"),
                    station_snapshot.runtime_state
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "运行时 enabled 覆盖", "Runtime enabled override"),
                    station_snapshot
                        .runtime_enabled_override
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "运行时 level 覆盖", "Runtime level override"),
                    station_snapshot
                        .runtime_level_override
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ));
                cols[1].label(format!(
                    "{}: {:?}",
                    pick(ctx.lang, "模型目录", "Model catalog"),
                    station_snapshot.capabilities.model_catalog_kind
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "支持 service tier", "Supports service tier"),
                    capability_support_label(
                        ctx.lang,
                        station_snapshot.capabilities.supports_service_tier
                    )
                ));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "支持 reasoning", "Supports reasoning"),
                    capability_support_label(
                        ctx.lang,
                        station_snapshot.capabilities.supports_reasoning_effort
                    )
                ));
                if !station_snapshot.capabilities.supported_models.is_empty() {
                    cols[1].small(format!(
                        "{}: {}",
                        pick(ctx.lang, "支持模型", "Supported models"),
                        station_snapshot.capabilities.supported_models.join(", ")
                    ));
                }
                cols[1].add_space(6.0);
                cols[1].horizontal(|ui| {
                    ui.checkbox(
                        &mut station_editor_enabled,
                        pick(ctx.lang, "启用", "Enabled"),
                    );
                    ui.label(pick(ctx.lang, "等级", "Level"));
                    ui.add(egui::DragValue::new(&mut station_editor_level).range(1..=10));
                });
            }

            cols[1].add_space(8.0);
            cols[1].separator();
            cols[1].label(pick(ctx.lang, "健康检查", "Health check"));
            if runtime_service.is_none() {
                cols[1].label(pick(
                    ctx.lang,
                    "代理未运行/未附着，无法执行健康检查。",
                    "Proxy is not running/attached; health check disabled.",
                ));
            } else if !supports_v1 {
                cols[1].label(pick(
                    ctx.lang,
                    "附着代理未启用 API v1：健康检查不可用。",
                    "Attached proxy has no API v1: health check disabled.",
                ));
            } else if runtime_service.as_deref() != Some(selected_service) {
                cols[1].label(pick(
                    ctx.lang,
                    "当前代理服务与所选服务不一致：健康检查已禁用。",
                    "Runtime service differs from selected service: health check disabled.",
                ));
            } else {
                if let Some(st) = hc_status.as_ref() {
                    cols[1].label(format!(
                        "status: {}/{} ok={} err={} cancel={} done={}",
                        st.completed, st.total, st.ok, st.err, st.cancel_requested, st.done
                    ));
                    if let Some(e) = st.last_error.as_deref() {
                        cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), e);
                    }
                } else {
                    cols[1].label(pick(ctx.lang, "(无状态)", "(no status)"));
                }

                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "探测当前", "Probe selected"))
                        .clicked()
                    {
                        action_probe_selected = Some(name.clone());
                    }
                    if ui
                        .button(pick(ctx.lang, "取消当前", "Cancel selected"))
                        .clicked()
                    {
                        action_health_cancel = Some((false, vec![name.clone()]));
                    }
                    if ui.button(pick(ctx.lang, "检查全部", "Check all")).clicked() {
                        action_health_start = Some((true, Vec::new()));
                    }
                    if ui
                        .button(pick(ctx.lang, "取消全部", "Cancel all"))
                        .clicked()
                    {
                        action_health_cancel = Some((true, Vec::new()));
                    }
                });

                if let Some(h) = cfg_health.as_ref() {
                    cols[1].add_space(6.0);
                    cols[1].label(format!(
                        "{}: {}  upstreams={}",
                        pick(ctx.lang, "最近检查", "Last checked"),
                        h.checked_at_ms,
                        h.upstreams.len()
                    ));
                }
            }

            if matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
                && !station_structure_control_plane_enabled
            {
                cols[1].add_space(6.0);
                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "设为 active_station", "Set active_station"))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(Some(name.clone()));
                        } else {
                            action_set_active = Some(name.clone());
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            "清除 active_station",
                            "Clear active_station",
                        ))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_set_active_remote = Some(None);
                        } else {
                            action_clear_active = true;
                        }
                    }

                    if ui
                        .button(pick(
                            ctx.lang,
                            if station_control_plane_enabled {
                                "保存到当前代理"
                            } else {
                                "保存并应用"
                            },
                            if station_control_plane_enabled {
                                "Save to current proxy"
                            } else {
                                "Save & apply"
                            },
                        ))
                        .clicked()
                    {
                        if station_control_plane_enabled {
                            action_save_apply_remote = Some((
                                name.clone(),
                                station_editor_enabled,
                                station_editor_level.clamp(1, 10),
                            ));
                        } else {
                            action_save_apply = true;
                        }
                    }
                });
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.group(|ui| {
            ui.heading(pick(ctx.lang, "Providers", "Providers"));
            ui.label(pick(
                ctx.lang,
                "Provider 负责认证引用与 endpoint 集合；适合做快捷切换、故障切换和不同中转站的结构管理。这里不会显示明文密钥。",
                "Providers hold auth references plus endpoint sets; they are the right place for quick switching, failover, and relay structure management. Plaintext secrets are never shown here.",
            ));

            if provider_structure_control_plane_enabled
                || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached)
            {
                ui.columns(2, |cols| {
                    cols[0].heading(pick(ctx.lang, "Provider 列表", "Provider list"));
                    cols[0].add_space(4.0);
                    cols[0].horizontal(|ui| {
                        ui.label(pick(ctx.lang, "新建 provider", "New provider"));
                        ui.add_sized(
                            [180.0, 22.0],
                            egui::TextEdit::singleline(&mut new_provider_name).hint_text(pick(
                                ctx.lang,
                                "例如 right / backup",
                                "e.g. right / backup",
                            )),
                        );
                        if ui
                            .add_enabled(
                                provider_structure_edit_enabled,
                                egui::Button::new(pick(ctx.lang, "新增", "Add")),
                            )
                            .clicked()
                        {
                            let name = new_provider_name.trim();
                            if name.is_empty() {
                                *ctx.last_error = Some(
                                    pick(
                                        ctx.lang,
                                        "provider 名称不能为空。",
                                        "Provider name cannot be empty.",
                                    )
                                    .to_string(),
                                );
                            } else if provider_structure_control_plane_enabled {
                                if attached_provider_specs
                                    .as_ref()
                                    .is_some_and(|providers| providers.contains_key(name))
                                {
                                    *ctx.last_error = Some(
                                        pick(
                                            ctx.lang,
                                            "provider 名称已存在。",
                                            "Provider name already exists.",
                                        )
                                        .to_string(),
                                    );
                                } else {
                                    action_upsert_provider_spec_remote = Some((
                                        name.to_string(),
                                        PersistedProviderSpec {
                                            name: name.to_string(),
                                            alias: None,
                                            enabled: true,
                                            auth_token_env: None,
                                            api_key_env: None,
                                            endpoints: Vec::new(),
                                        },
                                    ));
                                    selected_provider_name = Some(name.to_string());
                                    provider_editor_name = Some(name.to_string());
                                    provider_editor_alias.clear();
                                    provider_editor_enabled = true;
                                    provider_editor_auth_token_env.clear();
                                    provider_editor_api_key_env.clear();
                                    provider_editor_endpoints.clear();
                                    new_provider_name.clear();
                                }
                            } else if view.providers.contains_key(name) {
                                *ctx.last_error = Some(
                                    pick(
                                        ctx.lang,
                                        "provider 名称已存在。",
                                        "Provider name already exists.",
                                    )
                                    .to_string(),
                                );
                            } else {
                                view.providers.insert(name.to_string(), ProviderConfigV2::default());
                                selected_provider_name = Some(name.to_string());
                                provider_editor_name = Some(name.to_string());
                                provider_editor_alias.clear();
                                provider_editor_enabled = true;
                                provider_editor_auth_token_env.clear();
                                provider_editor_api_key_env.clear();
                                provider_editor_endpoints.clear();
                                new_provider_name.clear();
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已新增 provider（待保存）。",
                                        "Provider added (save pending).",
                                    )
                                    .to_string(),
                                );
                            }
                        }
                    });

                    egui::ScrollArea::vertical()
                        .id_salt("config_v2_providers_scroll")
                        .max_height(300.0)
                        .show(&mut cols[0], |ui| {
                            if provider_display_names.is_empty() {
                                ui.label(pick(
                                    ctx.lang,
                                    "当前没有 provider。可以先新增一个空 provider，再补 endpoint 与 env 引用。",
                                    "No providers yet. Add an empty provider first, then fill endpoints and env refs.",
                                ));
                            }
                            for name in provider_display_names.iter() {
                                let provider = if provider_structure_control_plane_enabled {
                                    attached_provider_specs
                                        .as_ref()
                                        .and_then(|providers| providers.get(name))
                                } else {
                                    local_provider_spec_catalog.get(name)
                                };
                                let (alias, enabled, endpoints) = provider
                                    .map(|provider| {
                                        (
                                            provider.alias.as_deref().unwrap_or(""),
                                            provider.enabled,
                                            provider.endpoints.len(),
                                        )
                                    })
                                    .unwrap_or(("", false, 0));
                                let mut label = format!("{name}  endpoints={endpoints}");
                                if !alias.trim().is_empty() {
                                    label.push_str(&format!(" ({alias})"));
                                }
                                if !enabled {
                                    label.push_str("  [off]");
                                }
                                if ui
                                    .selectable_label(
                                        selected_provider_name.as_deref() == Some(name.as_str()),
                                        label,
                                    )
                                    .clicked()
                                {
                                    selected_provider_name = Some(name.clone());
                                }
                            }
                        });

                    cols[1].heading(pick(ctx.lang, "Provider 详情", "Provider details"));
                    cols[1].add_space(4.0);

                    let Some(name) = selected_provider_name.clone() else {
                        cols[1].label(pick(ctx.lang, "未选择 provider。", "No provider selected."));
                        return;
                    };

                    let provider_snapshot = if provider_structure_control_plane_enabled {
                        let Some(provider) = attached_provider_specs
                            .as_ref()
                            .and_then(|providers| providers.get(name.as_str()))
                            .cloned()
                        else {
                            cols[1].label(pick(
                                ctx.lang,
                                "远端 provider 不存在（可能已被删除）。",
                                "Remote provider missing.",
                            ));
                            return;
                        };
                        provider
                    } else {
                        let Some(provider) = local_provider_spec_catalog.get(name.as_str()).cloned()
                        else {
                            cols[1].label(pick(
                                ctx.lang,
                                "provider 不存在（可能已被删除）。",
                                "Provider missing.",
                            ));
                            return;
                        };
                        provider
                    };

                    let referencing_stations = if provider_structure_control_plane_enabled {
                        attached_station_specs
                            .as_ref()
                            .map(|(stations, _)| {
                                stations
                                    .iter()
                                    .filter_map(|(station_name, station)| {
                                        station
                                            .members
                                            .iter()
                                            .any(|member| member.provider == name)
                                            .then_some(station_name.clone())
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    } else {
                        view.groups
                            .iter()
                            .filter_map(|(station_name, station)| {
                                station
                                    .members
                                    .iter()
                                    .any(|member| member.provider == name)
                                    .then_some(station_name.clone())
                            })
                            .collect::<Vec<_>>()
                    };

                    cols[1].colored_label(
                        egui::Color32::from_rgb(120, 120, 120),
                        if provider_structure_control_plane_enabled {
                            pick(
                                ctx.lang,
                                "当前通过附着代理暴露的 provider 结构 API 直接管理远端 provider；明文密钥、tags、模型映射等高级字段仍不会在这里暴露。",
                                "This view manages the attached proxy through its provider structure API directly; plaintext secrets, tags, and model mappings are still not exposed here.",
                            )
                        } else {
                            pick(
                                ctx.lang,
                                "这里编辑的是本机 v2 provider 结构；保存后会重载当前代理。高级 tags / model_mapping 仍建议在 Raw 视图处理。",
                                "This edits the local v2 provider structure; saving will reload the current proxy. Advanced tags and model_mapping are still better handled in Raw view.",
                            )
                        },
                    );
                    cols[1].add_space(6.0);
                    cols[1].label(format!("name: {name}"));
                    cols[1].label(format!("endpoints: {}", provider_snapshot.endpoints.len()));
                    cols[1].label(format!(
                        "stations: {}",
                        if referencing_stations.is_empty() {
                            "-".to_string()
                        } else {
                            referencing_stations.join(", ")
                        }
                    ));

                    cols[1].horizontal(|ui| {
                        ui.label("alias");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut provider_editor_alias),
                        );
                        if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                            provider_editor_alias.clear();
                        }
                    });

                    cols[1].horizontal(|ui| {
                        ui.checkbox(&mut provider_editor_enabled, pick(ctx.lang, "启用", "Enabled"));
                    });

                    cols[1].horizontal(|ui| {
                        ui.label("auth_token_env");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut provider_editor_auth_token_env),
                        );
                        if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                            provider_editor_auth_token_env.clear();
                        }
                    });

                    cols[1].horizontal(|ui| {
                        ui.label("api_key_env");
                        ui.add_sized(
                            [220.0, 22.0],
                            egui::TextEdit::singleline(&mut provider_editor_api_key_env),
                        );
                        if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                            provider_editor_api_key_env.clear();
                        }
                    });

                    cols[1].add_space(8.0);
                    cols[1].separator();
                    cols[1].label(pick(ctx.lang, "Endpoints", "Endpoints"));
                    render_config_provider_endpoint_editor(
                        &mut cols[1],
                        ctx.lang,
                        selected_service,
                        name.as_str(),
                        &mut provider_editor_endpoints,
                    );

                    cols[1].add_space(8.0);
                    cols[1].horizontal(|ui| {
                        if ui
                            .button(pick(
                                ctx.lang,
                                if provider_structure_control_plane_enabled {
                                    "保存到当前代理"
                                } else {
                                    "保存并应用"
                                },
                                if provider_structure_control_plane_enabled {
                                    "Save to current proxy"
                                } else {
                                    "Save & apply"
                                },
                            ))
                            .clicked()
                        {
                            match build_provider_spec_from_config_editor(
                                name.as_str(),
                                provider_editor_alias.as_str(),
                                provider_editor_enabled,
                                provider_editor_auth_token_env.as_str(),
                                provider_editor_api_key_env.as_str(),
                                &provider_editor_endpoints,
                            ) {
                                Ok(provider_spec) => {
                                    if provider_structure_control_plane_enabled {
                                        action_upsert_provider_spec_remote =
                                            Some((name.clone(), provider_spec));
                                    } else {
                                        let existing_provider =
                                            view.providers.get(name.as_str()).cloned();
                                        view.providers.insert(
                                            name.clone(),
                                            merge_provider_spec_into_provider_config(
                                                existing_provider.as_ref(),
                                                &provider_spec,
                                            ),
                                        );
                                        action_save_apply = true;
                                    }
                                }
                                Err(e) => {
                                    *ctx.last_error = Some(e);
                                }
                            }
                        }

                        if ui
                            .button(pick(ctx.lang, "删除 provider", "Delete provider"))
                            .clicked()
                        {
                            if !provider_structure_control_plane_enabled
                                && !referencing_stations.is_empty()
                            {
                                *ctx.last_error = Some(format!(
                                    "{}: {}",
                                    pick(
                                        ctx.lang,
                                        "仍有 station 引用了该 provider，不能删除",
                                        "Stations still reference this provider; delete is blocked",
                                    ),
                                    referencing_stations.join(", ")
                                ));
                            } else if provider_structure_control_plane_enabled {
                                action_delete_provider_spec_remote = Some(name.clone());
                            } else {
                                view.providers.remove(name.as_str());
                                selected_provider_name = view.providers.keys().next().cloned();
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已删除 provider（待保存）。",
                                        "Provider deleted (save pending).",
                                    )
                                    .to_string(),
                                );
                            }
                        }
                    });
                });
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(
                        ctx.lang,
                        "当前附着目标还没有暴露 provider 结构 API；这里保持只读，避免误导为会写回本机文件。需要查看高级字段时请使用 Raw 视图。",
                        "This attached target does not expose provider structure APIs yet; this section stays read-only to avoid implying local-file writes. Use Raw view for advanced fields.",
                    ),
                );
            }
        });

        ui.add_space(10.0);
        ui.separator();
        ui.group(|ui| {
            ui.heading(pick(ctx.lang, "Profiles", "Profiles"));
            ui.label(pick(
                ctx.lang,
                "Profile 用于把 station / model / reasoning_effort / service_tier 组合成可复用控制模板；更适合表达 fast mode、模型切换和思考模式。",
                "Profiles bundle station / model / reasoning_effort / service_tier into reusable control templates for fast mode, model switching, and reasoning mode.",
            ));
            if profile_control_plane_enabled {
                render_config_v2_profiles_control_plane(
                    ui,
                    ctx.lang,
                    selected_service,
                    &profile_control_plane_catalog,
                    profile_control_plane_default.as_deref(),
                    &profile_control_plane_station_names,
                    &mut selected_profile_name,
                    &mut new_profile_name,
                    &mut profile_editor_name,
                    &mut profile_editor_extends,
                    &mut profile_editor_station,
                    &mut profile_editor_model,
                    &mut profile_editor_reasoning_effort,
                    &mut profile_editor_service_tier,
                    &mut profile_error,
                    &mut action_profile_upsert_remote,
                    &mut action_profile_delete_remote,
                    &mut action_profile_set_persisted_default_remote,
                    matches!(ctx.proxy.kind(), ProxyModeKind::Attached),
                    station_control_plane_enabled,
                    station_control_plane_configured_active.as_deref(),
                    station_control_plane_effective_active.as_deref(),
                    preview_station_specs,
                    preview_provider_catalog,
                    preview_runtime_station_catalog,
                );
            } else {
            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "新建 profile", "New profile"));
                ui.add_sized(
                    [180.0, 22.0],
                    egui::TextEdit::singleline(&mut new_profile_name).hint_text(pick(
                        ctx.lang,
                        "例如 fast / deep / cheap",
                        "e.g. fast / deep / cheap",
                    )),
                );
                if ui.button(pick(ctx.lang, "新增", "Add")).clicked() {
                    let name = new_profile_name.trim();
                    if name.is_empty() {
                        profile_error = Some(
                            pick(
                                ctx.lang,
                                "profile 名称不能为空。",
                                "Profile name cannot be empty.",
                            )
                            .to_string(),
                        );
                    } else if view.profiles.contains_key(name) {
                        profile_error = Some(
                            pick(
                                ctx.lang,
                                "profile 名称已存在。",
                                "Profile name already exists.",
                            )
                            .to_string(),
                        );
                    } else {
                        view.profiles.insert(
                            name.to_string(),
                            crate::config::ServiceControlProfile::default(),
                        );
                        if view.default_profile.is_none() {
                            view.default_profile = Some(name.to_string());
                        }
                        selected_profile_name = Some(name.to_string());
                        new_profile_name.clear();
                        profile_info = Some(
                            pick(
                                ctx.lang,
                                "已新增 profile（待保存）。",
                                "Profile added (save pending).",
                            )
                            .to_string(),
                        );
                    }
                }
            });

            ui.add_space(6.0);
            ui.columns(2, |cols| {
                cols[0].label(pick(ctx.lang, "Profile 列表", "Profile list"));
                cols[0].add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("config_v2_profiles_scroll")
                    .max_height(240.0)
                    .show(&mut cols[0], |ui| {
                        let names = view.profiles.keys().cloned().collect::<Vec<_>>();
                        if names.is_empty() {
                            ui.label(pick(
                                ctx.lang,
                                "(当前没有 profile)",
                                "(no profiles yet)",
                            ));
                        } else {
                            for name in names {
                                let is_selected =
                                    selected_profile_name.as_deref() == Some(name.as_str());
                                let label = if view.default_profile.as_deref()
                                    == Some(name.as_str())
                                {
                                    format!("{name} [default]")
                                } else {
                                    name.clone()
                                };
                                if ui.selectable_label(is_selected, label).clicked() {
                                    selected_profile_name = Some(name);
                                }
                            }
                        }
                    });

                cols[1].label(pick(ctx.lang, "Profile 详情", "Profile details"));
                cols[1].add_space(4.0);

                let Some(profile_name) = selected_profile_name.clone() else {
                    cols[1].label(pick(
                        ctx.lang,
                        "未选择 profile。",
                        "No profile selected.",
                    ));
                    return;
                };

                let is_default = view.default_profile.as_deref() == Some(profile_name.as_str());
                let extends_candidates = view
                    .profiles
                    .keys()
                    .filter(|name| name.as_str() != profile_name.as_str())
                    .cloned()
                    .collect::<Vec<_>>();
                let mut preview_profile_catalog = view.profiles.clone();
                let mut delete_selected = false;
                let Some(profile) = view.profiles.get_mut(profile_name.as_str()) else {
                    cols[1].label(pick(
                        ctx.lang,
                        "profile 不存在（可能已被删除）。",
                        "Profile missing.",
                    ));
                    return;
                };

                cols[1].label(format!("name: {profile_name}"));
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "默认", "Default"),
                    if is_default {
                        pick(ctx.lang, "是", "yes")
                    } else {
                        pick(ctx.lang, "否", "no")
                    }
                ));

                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(ctx.lang, "设为 default_profile", "Set default_profile"))
                        .clicked()
                    {
                        view.default_profile = Some(profile_name.clone());
                        profile_info = Some(
                            pick(
                                ctx.lang,
                                "已更新 default_profile（待保存）。",
                                "default_profile updated (save pending).",
                            )
                            .to_string(),
                        );
                    }
                    if ui
                        .button(pick(ctx.lang, "清除 default_profile", "Clear default_profile"))
                        .clicked()
                    {
                        if is_default {
                            view.default_profile = None;
                            profile_info = Some(
                                pick(
                                    ctx.lang,
                                    "已清除 default_profile（待保存）。",
                                    "default_profile cleared (save pending).",
                                )
                                .to_string(),
                            );
                        }
                    }
                    if ui.button(pick(ctx.lang, "删除 profile", "Delete profile")).clicked() {
                        delete_selected = true;
                    }
                });

                let mut extends = profile.extends.clone();
                cols[1].horizontal(|ui| {
                    ui.label("extends");
                    egui::ComboBox::from_id_salt(format!(
                        "config_v2_profile_extends_{selected_service}_{profile_name}"
                    ))
                    .selected_text(extends.as_deref().unwrap_or("<none>"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut extends, None, "<none>");
                        for extends_name in extends_candidates.iter() {
                            ui.selectable_value(
                                &mut extends,
                                Some(extends_name.clone()),
                                extends_name.as_str(),
                            );
                        }
                    });
                });
                if extends != profile.extends {
                    profile.extends = extends;
                }

                let mut station = profile.station.clone();
                cols[1].horizontal(|ui| {
                    ui.label(pick(ctx.lang, "station", "station"));
                    egui::ComboBox::from_id_salt(format!(
                        "config_v2_profile_station_{selected_service}_{profile_name}"
                    ))
                    .selected_text(
                        station
                            .as_deref()
                            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>")),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut station,
                            None,
                            pick(ctx.lang, "<自动>", "<auto>"),
                        );
                        for station_name in station_names.iter() {
                            ui.selectable_value(
                                &mut station,
                                Some(station_name.clone()),
                                station_name.as_str(),
                            );
                        }
                    });
                });
                if station != profile.station {
                    profile.station = station;
                }

                let mut model = profile.model.clone().unwrap_or_default();
                cols[1].horizontal(|ui| {
                    ui.label("model");
                    ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(&mut model));
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        model.clear();
                    }
                });
                let next_model = non_empty_trimmed(Some(model.as_str()));
                if next_model != profile.model {
                    profile.model = next_model;
                }

                let mut effort = profile.reasoning_effort.clone().unwrap_or_default();
                cols[1].horizontal(|ui| {
                    ui.label("reasoning_effort");
                    ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(&mut effort));
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        effort.clear();
                    }
                });
                let next_effort = non_empty_trimmed(Some(effort.as_str()));
                if next_effort != profile.reasoning_effort {
                    profile.reasoning_effort = next_effort;
                }

                let mut tier = profile.service_tier.clone().unwrap_or_default();
                cols[1].horizontal(|ui| {
                    ui.label("service_tier");
                    ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(&mut tier));
                    if ui.button(pick(ctx.lang, "清除", "Clear")).clicked() {
                        tier.clear();
                    }
                });
                let next_tier = non_empty_trimmed(Some(tier.as_str()));
                if next_tier != profile.service_tier {
                    profile.service_tier = next_tier;
                }
                let declared_profile = profile.clone();

                cols[1].add_space(6.0);
                cols[1].small(format_profile_summary(&ControlProfileOption {
                    name: profile_name.clone(),
                    extends: declared_profile.extends.clone(),
                    station: declared_profile.station.clone(),
                    model: declared_profile.model.clone(),
                    reasoning_effort: declared_profile.reasoning_effort.clone(),
                    service_tier: declared_profile.service_tier.clone(),
                    is_default,
                }));
                cols[1].small(pick(
                    ctx.lang,
                    "提示：service_tier=priority 通常可视为 fast mode；reasoning_effort 可表达思考模式。",
                    "Tip: service_tier=priority usually maps to fast mode; reasoning_effort expresses reasoning mode.",
                ));
                preview_profile_catalog.insert(profile_name.clone(), declared_profile.clone());
                let preview_profile = match crate::config::resolve_service_profile_from_catalog(
                    &preview_profile_catalog,
                    profile_name.as_str(),
                ) {
                    Ok(profile) => profile,
                    Err(err) => {
                        cols[1].small(format!(
                            "{} {err}",
                            pick(
                                ctx.lang,
                                "profile 预览解析失败：",
                                "Profile preview resolve failed:",
                            )
                        ));
                        declared_profile.clone()
                    }
                };
                let profile_preview = build_profile_route_preview(
                    &preview_profile,
                    configured_active_name.as_deref(),
                    effective_active_name.as_deref(),
                    preview_station_specs,
                    preview_provider_catalog,
                    preview_runtime_station_catalog,
                );
                render_profile_route_preview(
                    &mut cols[1],
                    ctx.lang,
                    &preview_profile,
                    &profile_preview,
                );

                if delete_selected {
                    view.profiles.remove(profile_name.as_str());
                    if view.default_profile.as_deref() == Some(profile_name.as_str()) {
                        view.default_profile = None;
                    }
                    selected_profile_name = view
                        .default_profile
                        .clone()
                        .or_else(|| view.profiles.keys().next().cloned());
                    profile_info = Some(
                        pick(
                            ctx.lang,
                            "已删除 profile（待保存）。",
                            "Profile deleted (save pending).",
                        )
                        .to_string(),
                    );
                }
            });

            ui.add_space(6.0);
            if ui
                .button(pick(ctx.lang, "保存并应用 profile 变更", "Save & apply profile changes"))
                .clicked()
            {
                action_save_apply = true;
            }
            }
        });
    }

    ctx.view.config.selected_provider_name = selected_provider_name;
    ctx.view.config.selected_profile_name = selected_profile_name;
    ctx.view.config.new_profile_name = new_profile_name;
    ctx.view.config.station_editor.station_name = station_editor_name;
    ctx.view.config.station_editor.alias = station_editor_alias;
    ctx.view.config.station_editor.enabled = station_editor_enabled;
    ctx.view.config.station_editor.level = station_editor_level.clamp(1, 10);
    ctx.view.config.station_editor.members = station_editor_members;
    ctx.view.config.station_editor.new_station_name = new_station_name;
    ctx.view.config.provider_editor.provider_name = provider_editor_name;
    ctx.view.config.provider_editor.alias = provider_editor_alias;
    ctx.view.config.provider_editor.enabled = provider_editor_enabled;
    ctx.view.config.provider_editor.auth_token_env = provider_editor_auth_token_env;
    ctx.view.config.provider_editor.api_key_env = provider_editor_api_key_env;
    ctx.view.config.provider_editor.endpoints = provider_editor_endpoints;
    ctx.view.config.provider_editor.new_provider_name = new_provider_name;
    ctx.view.config.profile_editor.profile_name = profile_editor_name;
    ctx.view.config.profile_editor.extends = profile_editor_extends;
    ctx.view.config.profile_editor.station = profile_editor_station;
    ctx.view.config.profile_editor.model = profile_editor_model;
    ctx.view.config.profile_editor.reasoning_effort = profile_editor_reasoning_effort;
    ctx.view.config.profile_editor.service_tier = profile_editor_service_tier;
    if let Some(message) = profile_info {
        *ctx.last_info = Some(message);
    }
    if let Some(message) = profile_error {
        *ctx.last_error = Some(message);
    }

    if let Some((profile_name, profile)) = action_profile_upsert_remote {
        match ctx
            .proxy
            .upsert_persisted_profile(ctx.rt, profile_name.clone(), profile)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some(default_profile_name) = action_profile_set_persisted_default_remote {
        match ctx
            .proxy
            .set_persisted_default_profile(ctx.rt, default_profile_name.clone())
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some(profile_name) = action_profile_delete_remote {
        match ctx.proxy.delete_persisted_profile(ctx.rt, profile_name) {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some(station_name) = action_set_active_remote {
        match ctx
            .proxy
            .set_persisted_active_station(ctx.rt, station_name.clone())
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some((station_name, enabled, level)) = action_save_apply_remote {
        match ctx
            .proxy
            .update_persisted_station(ctx.rt, station_name, enabled, level)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some((station_name, station_spec)) = action_upsert_station_spec_remote {
        match ctx
            .proxy
            .upsert_persisted_station_spec(ctx.rt, station_name.clone(), station_spec)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some(station_name) = action_delete_station_spec_remote {
        match ctx
            .proxy
            .delete_persisted_station_spec(ctx.rt, station_name)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some((provider_name, provider_spec)) = action_upsert_provider_spec_remote {
        match ctx
            .proxy
            .upsert_persisted_provider_spec(ctx.rt, provider_name.clone(), provider_spec)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some(provider_name) = action_delete_provider_spec_remote {
        match ctx
            .proxy
            .delete_persisted_provider_spec(ctx.rt, provider_name)
        {
            Ok(()) => {
                ctx.proxy
                    .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
                refresh_config_editor_from_disk_if_running(ctx);
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

    if let Some(name) = action_set_active {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
            return;
        };
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        view.active_group = Some(name);
        *ctx.last_info =
            Some(pick(ctx.lang, "已设置 active_station", "active_station set").to_string());
    }

    if action_clear_active {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_mut() else {
            return;
        };
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &mut cfg.claude,
            crate::config::ServiceKind::Codex => &mut cfg.codex,
        };
        view.active_group = None;
        *ctx.last_info =
            Some(pick(ctx.lang, "已清除 active_station", "active_station cleared").to_string());
    }

    if let Some((all, names)) = action_health_start {
        if let Err(e) = ctx.proxy.start_health_checks(ctx.rt, all, names) {
            *ctx.last_error = Some(format!("health check start failed: {e}"));
        } else {
            *ctx.last_info =
                Some(pick(ctx.lang, "已开始健康检查", "Health check started").to_string());
        }
    }

    if let Some(name) = action_probe_selected {
        if let Err(e) = ctx.proxy.probe_station(ctx.rt, name) {
            *ctx.last_error = Some(format!("station probe failed: {e}"));
        } else {
            *ctx.last_info = Some(pick(ctx.lang, "已开始探测", "Probe started").to_string());
        }
    }

    if let Some((all, names)) = action_health_cancel {
        if let Err(e) = ctx.proxy.cancel_health_checks(ctx.rt, all, names) {
            *ctx.last_error = Some(format!("health check cancel failed: {e}"));
        } else {
            *ctx.last_info = Some(pick(ctx.lang, "已请求取消", "Cancel requested").to_string());
        }
    }

    if action_save_apply {
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
                    ProxyModeKind::Running
                        | ProxyModeKind::Attached
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
}


pub(super) fn config_station_member_editor_from_member(
    member: &GroupMemberRefV2,
) -> ConfigStationMemberEditorState {
    ConfigStationMemberEditorState {
        provider: member.provider.clone(),
        endpoint_names: member.endpoint_names.join(", "),
        preferred: member.preferred,
    }
}

fn parse_station_member_endpoint_names(raw: &str) -> Vec<String> {
    let mut out = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    out.dedup();
    out
}

pub(super) fn build_station_spec_from_config_editor(
    station_name: &str,
    alias: &str,
    enabled: bool,
    level: u8,
    members: &[ConfigStationMemberEditorState],
) -> Result<PersistedStationSpec, String> {
    let station_name = station_name.trim();
    if station_name.is_empty() {
        return Err("station name is required".to_string());
    }

    let mut spec_members = Vec::new();
    for (index, member) in members.iter().enumerate() {
        let provider = member.provider.trim();
        if provider.is_empty() {
            return Err(format!("member #{} provider is required", index + 1));
        }
        spec_members.push(GroupMemberRefV2 {
            provider: provider.to_string(),
            endpoint_names: parse_station_member_endpoint_names(member.endpoint_names.as_str()),
            preferred: member.preferred,
        });
    }

    Ok(PersistedStationSpec {
        name: station_name.to_string(),
        alias: non_empty_trimmed(Some(alias)),
        enabled,
        level: level.clamp(1, 10),
        members: spec_members,
    })
}

pub(super) fn render_config_station_member_editor(
    ui: &mut egui::Ui,
    lang: Language,
    selected_service: &str,
    provider_catalog: &BTreeMap<String, PersistedStationProviderRef>,
    members: &mut Vec<ConfigStationMemberEditorState>,
) {
    let default_provider = provider_catalog.keys().next().cloned().unwrap_or_default();

    if ui.button(pick(lang, "新增成员", "Add member")).clicked() {
        members.push(ConfigStationMemberEditorState {
            provider: default_provider,
            endpoint_names: String::new(),
            preferred: false,
        });
    }

    egui::ScrollArea::vertical()
        .id_salt(format!("config_v2_station_members_edit_{selected_service}"))
        .max_height(180.0)
        .show(ui, |ui| {
            if members.is_empty() {
                ui.label(pick(
                    lang,
                    "(无成员；可先保存空 station，再逐步补引用)",
                    "(no members yet; you can save an empty station first and fill refs later)",
                ));
                return;
            }

            let mut delete_idx = None;
            for (idx, member) in members.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("#{}", idx + 1));
                        ui.checkbox(&mut member.preferred, pick(lang, "preferred", "preferred"));
                        if ui.button(pick(lang, "删除", "Delete")).clicked() {
                            delete_idx = Some(idx);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("provider");
                        egui::ComboBox::from_id_salt(format!(
                            "config_v2_station_member_provider_{selected_service}_{idx}"
                        ))
                        .selected_text(if member.provider.trim().is_empty() {
                            pick(lang, "<未选择>", "<unset>")
                        } else {
                            member.provider.as_str()
                        })
                        .show_ui(ui, |ui| {
                            if provider_catalog.is_empty() {
                                ui.label(pick(lang, "(无 provider)", "(no providers)"));
                            } else {
                                for provider_name in provider_catalog.keys() {
                                    ui.selectable_value(
                                        &mut member.provider,
                                        provider_name.clone(),
                                        provider_name.as_str(),
                                    );
                                }
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("endpoint_names");
                        ui.add_sized(
                            [240.0, 22.0],
                            egui::TextEdit::singleline(&mut member.endpoint_names).hint_text(pick(
                                lang,
                                "空=provider 下全部 endpoint；或填 default,hk",
                                "empty=all provider endpoints; or enter default,hk",
                            )),
                        );
                    });
                });
                ui.add_space(4.0);
            }

            if let Some(idx) = delete_idx {
                members.remove(idx);
            }
        });
}

pub(super) fn render_config_station_provider_summary(
    ui: &mut egui::Ui,
    lang: Language,
    provider_catalog: &BTreeMap<String, PersistedStationProviderRef>,
    members: &[ConfigStationMemberEditorState],
) {
    let mut provider_names = members
        .iter()
        .map(|member| member.provider.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    provider_names.sort();
    provider_names.dedup();

    if provider_names.is_empty() {
        provider_names = provider_catalog.keys().cloned().collect();
    }

    egui::ScrollArea::vertical()
        .id_salt("config_v2_station_provider_summary")
        .max_height(140.0)
        .show(ui, |ui| {
            if provider_names.is_empty() {
                ui.label(pick(lang, "(无 provider)", "(no providers)"));
                return;
            }
            for provider_name in provider_names {
                let Some(provider) = provider_catalog.get(provider_name.as_str()) else {
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 120, 40),
                        format!("missing provider: {provider_name}"),
                    );
                    continue;
                };
                ui.label(format!(
                    "{}  alias={}  endpoints={}  enabled={}",
                    provider.name,
                    provider.alias.as_deref().unwrap_or("-"),
                    provider.endpoints.len(),
                    provider.enabled
                ));
                if !provider.endpoints.is_empty() {
                    ui.small(
                        provider
                            .endpoints
                            .iter()
                            .map(|endpoint| {
                                format!(
                                    "{}={}",
                                    endpoint.name,
                                    shorten_middle(&endpoint.base_url, 48)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                }
                ui.add_space(4.0);
            }
        });
}

pub(super) fn config_provider_endpoint_editor_from_spec(
    endpoint: &crate::config::PersistedProviderEndpointSpec,
) -> ConfigProviderEndpointEditorState {
    ConfigProviderEndpointEditorState {
        name: endpoint.name.clone(),
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
    }
}

pub(super) fn build_provider_spec_from_config_editor(
    provider_name: &str,
    alias: &str,
    enabled: bool,
    auth_token_env: &str,
    api_key_env: &str,
    endpoints: &[ConfigProviderEndpointEditorState],
) -> Result<PersistedProviderSpec, String> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return Err("provider name is required".to_string());
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut spec_endpoints = Vec::new();
    for (index, endpoint) in endpoints.iter().enumerate() {
        let endpoint_name = endpoint.name.trim();
        if endpoint_name.is_empty() {
            return Err(format!("endpoint #{} name is required", index + 1));
        }
        if !seen.insert(endpoint_name.to_string()) {
            return Err(format!("duplicate endpoint name: {endpoint_name}"));
        }
        let base_url = endpoint.base_url.trim();
        if base_url.is_empty() {
            return Err(format!("endpoint '{}' base_url is required", endpoint_name));
        }
        spec_endpoints.push(crate::config::PersistedProviderEndpointSpec {
            name: endpoint_name.to_string(),
            base_url: base_url.to_string(),
            enabled: endpoint.enabled,
        });
    }

    Ok(PersistedProviderSpec {
        name: provider_name.to_string(),
        alias: non_empty_trimmed(Some(alias)),
        enabled,
        auth_token_env: non_empty_trimmed(Some(auth_token_env)),
        api_key_env: non_empty_trimmed(Some(api_key_env)),
        endpoints: spec_endpoints,
    })
}

pub(super) fn merge_provider_spec_into_provider_config(
    existing: Option<&ProviderConfigV2>,
    provider: &PersistedProviderSpec,
) -> ProviderConfigV2 {
    let mut auth = existing
        .map(|provider| provider.auth.clone())
        .unwrap_or_default();
    auth.auth_token_env = provider.auth_token_env.clone();
    auth.api_key_env = provider.api_key_env.clone();

    ProviderConfigV2 {
        alias: provider.alias.clone(),
        enabled: provider.enabled,
        auth,
        tags: existing
            .map(|provider| provider.tags.clone())
            .unwrap_or_default(),
        supported_models: existing
            .map(|provider| provider.supported_models.clone())
            .unwrap_or_default(),
        model_mapping: existing
            .map(|provider| provider.model_mapping.clone())
            .unwrap_or_default(),
        endpoints: provider
            .endpoints
            .iter()
            .map(|endpoint| {
                let existing_endpoint =
                    existing.and_then(|provider| provider.endpoints.get(endpoint.name.as_str()));
                (
                    endpoint.name.clone(),
                    ProviderEndpointV2 {
                        base_url: endpoint.base_url.clone(),
                        enabled: endpoint.enabled,
                        tags: existing_endpoint
                            .map(|endpoint| endpoint.tags.clone())
                            .unwrap_or_default(),
                        supported_models: existing_endpoint
                            .map(|endpoint| endpoint.supported_models.clone())
                            .unwrap_or_default(),
                        model_mapping: existing_endpoint
                            .map(|endpoint| endpoint.model_mapping.clone())
                            .unwrap_or_default(),
                    },
                )
            })
            .collect(),
    }
}

pub(super) fn render_config_provider_endpoint_editor(
    ui: &mut egui::Ui,
    lang: Language,
    selected_service: &str,
    provider_name: &str,
    endpoints: &mut Vec<ConfigProviderEndpointEditorState>,
) {
    if ui.button(pick(lang, "新增 endpoint", "Add endpoint")).clicked() {
        endpoints.push(ConfigProviderEndpointEditorState {
            enabled: true,
            ..Default::default()
        });
    }

    egui::ScrollArea::vertical()
        .id_salt(format!(
            "config_v2_provider_endpoints_edit_{selected_service}_{provider_name}"
        ))
        .max_height(180.0)
        .show(ui, |ui| {
            if endpoints.is_empty() {
                ui.label(pick(
                    lang,
                    "(无 endpoint；可先保存空 provider，再逐步补地址)",
                    "(no endpoints yet; you can save an empty provider first and fill URLs later)",
                ));
                return;
            }

            let mut delete_idx = None;
            for (idx, endpoint) in endpoints.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!("#{}", idx + 1));
                        ui.checkbox(&mut endpoint.enabled, pick(lang, "启用", "Enabled"));
                        if ui.button(pick(lang, "删除", "Delete")).clicked() {
                            delete_idx = Some(idx);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("name");
                        ui.add_sized(
                            [180.0, 22.0],
                            egui::TextEdit::singleline(&mut endpoint.name),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("base_url");
                        ui.add_sized(
                            [280.0, 22.0],
                            egui::TextEdit::singleline(&mut endpoint.base_url)
                                .hint_text("https://example.com/v1"),
                        );
                    });
                });
                ui.add_space(4.0);
            }

            if let Some(idx) = delete_idx {
                endpoints.remove(idx);
            }
        });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_config_v2_profiles_control_plane(
    ui: &mut egui::Ui,
    lang: Language,
    selected_service: &str,
    profile_catalog: &BTreeMap<String, crate::config::ServiceControlProfile>,
    configured_default_profile: Option<&str>,
    station_names: &[String],
    selected_profile_name: &mut Option<String>,
    new_profile_name: &mut String,
    editor_profile_name: &mut Option<String>,
    editor_extends: &mut Option<String>,
    editor_station: &mut Option<String>,
    editor_model: &mut String,
    editor_reasoning_effort: &mut String,
    editor_service_tier: &mut String,
    profile_error: &mut Option<String>,
    action_profile_upsert_remote: &mut Option<(String, crate::config::ServiceControlProfile)>,
    action_profile_delete_remote: &mut Option<String>,
    action_profile_set_persisted_default_remote: &mut Option<Option<String>>,
    attached_mode: bool,
    station_control_plane_enabled: bool,
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
    preview_station_specs: Option<&BTreeMap<String, PersistedStationSpec>>,
    preview_provider_catalog: Option<&BTreeMap<String, PersistedStationProviderRef>>,
    preview_runtime_station_catalog: Option<&BTreeMap<String, StationOption>>,
) {
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        if attached_mode {
            if station_control_plane_enabled {
                pick(
                    lang,
                    "当前 station 常用字段与下面的 Profiles 都直接管理附着代理；provider/member 深层结构仍建议在原始视图查看。",
                    "Station common fields and the Profiles below manage the attached proxy directly; use Raw view for deeper provider/member structure.",
                )
            } else {
                pick(
                    lang,
                    "下面的 Profiles 直接管理当前附着代理；上面的 station/provider 仍然是本机文件视图。",
                    "Profiles below manage the attached proxy directly; the station/provider form above still reflects the local file on this device.",
                )
            }
        } else {
            pick(
                lang,
                "下面的 Profiles 直接管理当前运行中的代理配置。",
                "Profiles below manage the currently running proxy config directly.",
            )
        },
    );

    ui.horizontal(|ui| {
        ui.label(pick(lang, "新建 profile", "New profile"));
        ui.add_sized(
            [180.0, 22.0],
            egui::TextEdit::singleline(new_profile_name).hint_text(pick(
                lang,
                "例如 fast / deep / cheap",
                "e.g. fast / deep / cheap",
            )),
        );
        if ui.button(pick(lang, "新增", "Add")).clicked() {
            let name = new_profile_name.trim();
            if name.is_empty() {
                *profile_error = Some(
                    pick(lang, "profile 名称不能为空。", "Profile name cannot be empty.")
                        .to_string(),
                );
            } else if profile_catalog.contains_key(name) {
                *profile_error = Some(
                    pick(lang, "profile 名称已存在。", "Profile name already exists.").to_string(),
                );
            } else {
                *action_profile_upsert_remote = Some((
                    name.to_string(),
                    crate::config::ServiceControlProfile::default(),
                ));
                if configured_default_profile.is_none() {
                    *action_profile_set_persisted_default_remote = Some(Some(name.to_string()));
                }
                *selected_profile_name = Some(name.to_string());
                *editor_profile_name = Some(name.to_string());
                *editor_extends = None;
                *editor_station = None;
                editor_model.clear();
                editor_reasoning_effort.clear();
                editor_service_tier.clear();
                new_profile_name.clear();
            }
        }
    });

    ui.add_space(6.0);
    ui.columns(2, |cols| {
        cols[0].label(pick(lang, "Profile 列表", "Profile list"));
        cols[0].add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("config_v2_profiles_scroll")
            .max_height(240.0)
            .show(&mut cols[0], |ui| {
                if profile_catalog.is_empty() {
                    ui.label(pick(lang, "(当前没有 profile)", "(no profiles yet)"));
                } else {
                    for name in profile_catalog.keys() {
                        let is_selected = selected_profile_name.as_deref() == Some(name.as_str());
                        let label = if configured_default_profile == Some(name.as_str()) {
                            format!("{name} [default]")
                        } else {
                            name.clone()
                        };
                        if ui.selectable_label(is_selected, label).clicked() {
                            *selected_profile_name = Some(name.clone());
                        }
                    }
                }
            });

        if editor_profile_name.as_deref() != selected_profile_name.as_deref() {
            let selected_profile = selected_profile_name
                .as_deref()
                .and_then(|name| profile_catalog.get(name));
            *editor_profile_name = selected_profile_name.clone();
            *editor_extends = selected_profile.and_then(|profile| profile.extends.clone());
            *editor_station = selected_profile.and_then(|profile| profile.station.clone());
            *editor_model = selected_profile
                .and_then(|profile| profile.model.clone())
                .unwrap_or_default();
            *editor_reasoning_effort = selected_profile
                .and_then(|profile| profile.reasoning_effort.clone())
                .unwrap_or_default();
            *editor_service_tier = selected_profile
                .and_then(|profile| profile.service_tier.clone())
                .unwrap_or_default();
        }

        cols[1].label(pick(lang, "Profile 详情", "Profile details"));
        cols[1].add_space(4.0);

        let Some(profile_name) = selected_profile_name.clone() else {
            cols[1].label(pick(lang, "未选择 profile。", "No profile selected."));
            return;
        };

        let Some(profile) = profile_catalog.get(profile_name.as_str()) else {
            cols[1].label(pick(lang, "profile 不存在（可能已被删除）。", "Profile missing."));
            return;
        };
        let is_default = configured_default_profile == Some(profile_name.as_str());
        let extends_candidates = profile_catalog
            .keys()
            .filter(|name| name.as_str() != profile_name.as_str())
            .cloned()
            .collect::<Vec<_>>();
        let mut preview_profile_catalog = profile_catalog.clone();

        cols[1].label(format!("name: {profile_name}"));
        cols[1].label(format!(
            "{}: {}",
            pick(lang, "默认", "Default"),
            if is_default {
                pick(lang, "是", "yes")
            } else {
                pick(lang, "否", "no")
            }
        ));

        cols[1].horizontal(|ui| {
            if ui
                .button(pick(lang, "设为 default_profile", "Set default_profile"))
                .clicked()
            {
                *action_profile_set_persisted_default_remote = Some(Some(profile_name.clone()));
            }
            if ui
                .button(pick(lang, "清除 default_profile", "Clear default_profile"))
                .clicked()
                && is_default
            {
                *action_profile_set_persisted_default_remote = Some(None);
            }
            if ui.button(pick(lang, "删除 profile", "Delete profile")).clicked() {
                *action_profile_delete_remote = Some(profile_name.clone());
            }
        });

        cols[1].horizontal(|ui| {
            ui.label("extends");
            egui::ComboBox::from_id_salt(format!(
                "config_v2_profile_extends_remote_{selected_service}_{profile_name}"
            ))
            .selected_text(editor_extends.as_deref().unwrap_or("<none>"))
            .show_ui(ui, |ui| {
                ui.selectable_value(editor_extends, None, "<none>");
                for extends_name in extends_candidates.iter() {
                    ui.selectable_value(
                        editor_extends,
                        Some(extends_name.clone()),
                        extends_name.as_str(),
                    );
                }
            });
        });

        cols[1].horizontal(|ui| {
            ui.label(pick(lang, "station", "station"));
            egui::ComboBox::from_id_salt(format!(
                "config_v2_profile_station_remote_{selected_service}_{profile_name}"
            ))
            .selected_text(
                editor_station
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "<自动>", "<auto>")),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(editor_station, None, pick(lang, "<自动>", "<auto>"));
                for station_name in station_names {
                    ui.selectable_value(
                        editor_station,
                        Some(station_name.clone()),
                        station_name.as_str(),
                    );
                }
            });
        });

        cols[1].horizontal(|ui| {
            ui.label("model");
            ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(editor_model));
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
                editor_model.clear();
            }
        });

        cols[1].horizontal(|ui| {
            ui.label("reasoning_effort");
            ui.add_sized(
                [220.0, 22.0],
                egui::TextEdit::singleline(editor_reasoning_effort),
            );
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
                editor_reasoning_effort.clear();
            }
        });

        cols[1].horizontal(|ui| {
            ui.label("service_tier");
            ui.add_sized(
                [220.0, 22.0],
                egui::TextEdit::singleline(editor_service_tier),
            );
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
                editor_service_tier.clear();
            }
        });

        let declared_profile = crate::config::ServiceControlProfile {
            extends: editor_extends.clone(),
            station: editor_station.clone(),
            model: non_empty_trimmed(Some(editor_model.as_str())),
            reasoning_effort: non_empty_trimmed(Some(editor_reasoning_effort.as_str())),
            service_tier: non_empty_trimmed(Some(editor_service_tier.as_str())),
        };
        cols[1].add_space(6.0);
        cols[1].small(format_profile_summary(&ControlProfileOption {
            name: profile_name.clone(),
            extends: declared_profile.extends.clone(),
            station: declared_profile.station.clone(),
            model: declared_profile.model.clone(),
            reasoning_effort: declared_profile.reasoning_effort.clone(),
            service_tier: declared_profile.service_tier.clone(),
            is_default,
        }));
        cols[1].small(pick(
            lang,
            "提示：service_tier=priority 通常可视为 fast mode；reasoning_effort 可表达思考模式。",
            "Tip: service_tier=priority usually maps to fast mode; reasoning_effort expresses reasoning mode.",
        ));
        preview_profile_catalog.insert(profile_name.clone(), declared_profile.clone());
        let preview_profile = match crate::config::resolve_service_profile_from_catalog(
            &preview_profile_catalog,
            profile_name.as_str(),
        ) {
            Ok(profile) => profile,
            Err(err) => {
                cols[1].small(format!(
                    "{} {err}",
                    pick(lang, "profile 预览解析失败：", "Profile preview resolve failed:")
                ));
                declared_profile.clone()
            }
        };
        let profile_preview = build_profile_route_preview(
            &preview_profile,
            configured_active_station,
            effective_active_station,
            preview_station_specs,
            preview_provider_catalog,
            preview_runtime_station_catalog,
        );
        render_profile_route_preview(&mut cols[1], lang, &preview_profile, &profile_preview);
        if editor_extends != &profile.extends
            || editor_station != &profile.station
            || non_empty_trimmed(Some(editor_model.as_str())) != profile.model
            || non_empty_trimmed(Some(editor_reasoning_effort.as_str()))
                != profile.reasoning_effort
            || non_empty_trimmed(Some(editor_service_tier.as_str())) != profile.service_tier
        {
            cols[1].small(pick(
                lang,
                "当前编辑内容尚未写入代理配置。",
                "Current edits have not been written to the proxy config yet.",
            ));
        }
    });

    ui.add_space(6.0);
    if ui
        .button(pick(
            lang,
            "保存并应用 profile 变更",
            "Save & apply profile changes",
        ))
        .clicked()
        && let Some(profile_name) = selected_profile_name.clone()
    {
        *action_profile_upsert_remote = Some((
            profile_name,
            crate::config::ServiceControlProfile {
                extends: editor_extends.clone(),
                station: editor_station.clone(),
                model: non_empty_trimmed(Some(editor_model.as_str())),
                reasoning_effort: non_empty_trimmed(Some(editor_reasoning_effort.as_str())),
                service_tier: non_empty_trimmed(Some(editor_service_tier.as_str())),
            },
        ));
    }
}
