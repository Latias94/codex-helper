use super::*;

mod editors;

use editors::*;

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
            runtime_mgr.and_then(|mgr| mgr.active_station().map(|cfg| cfg.name.clone())),
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

        render_config_v2_stations_section(
            ui,
            StationsSectionArgs {
                lang: ctx.lang,
                proxy_kind: ctx.proxy.kind(),
                last_error: ctx.last_error,
                last_info: ctx.last_info,
                view,
                selected_service,
                schema_version,
                station_display_names: &station_display_names,
                selected_name: &mut ctx.view.config.selected_name,
                station_control_plane_enabled,
                station_structure_control_plane_enabled,
                station_structure_edit_enabled,
                station_control_plane_catalog: &station_control_plane_catalog,
                configured_active_name: configured_active_name.clone(),
                effective_active_name: effective_active_name.clone(),
                station_default_profile,
                attached_station_specs: attached_station_specs.as_ref(),
                local_station_spec_catalog: &local_station_spec_catalog,
                local_provider_ref_catalog: &local_provider_ref_catalog,
                provider_catalog: &provider_catalog,
                profile_catalog: &profile_catalog,
                runtime_service: runtime_service.as_deref(),
                supports_v1,
                cfg_health: cfg_health.as_ref(),
                hc_status: hc_status.as_ref(),
                action_set_active: &mut action_set_active,
                action_clear_active: &mut action_clear_active,
                action_set_active_remote: &mut action_set_active_remote,
                action_save_apply: &mut action_save_apply,
                action_save_apply_remote: &mut action_save_apply_remote,
                action_upsert_station_spec_remote: &mut action_upsert_station_spec_remote,
                action_delete_station_spec_remote: &mut action_delete_station_spec_remote,
                action_probe_selected: &mut action_probe_selected,
                action_health_start: &mut action_health_start,
                action_health_cancel: &mut action_health_cancel,
                new_station_name: &mut new_station_name,
                station_editor_name: &mut station_editor_name,
                station_editor_alias: &mut station_editor_alias,
                station_editor_enabled: &mut station_editor_enabled,
                station_editor_level: &mut station_editor_level,
                station_editor_members: &mut station_editor_members,
            },
        );

        ui.add_space(10.0);
        ui.separator();
        render_config_v2_providers_section(
            ui,
            ctx.lang,
            ctx.proxy.kind(),
            ctx.last_error,
            ctx.last_info,
            view,
            selected_service,
            provider_structure_control_plane_enabled,
            provider_structure_edit_enabled,
            attached_provider_specs.as_ref(),
            attached_station_specs.as_ref(),
            &local_provider_spec_catalog,
            &provider_display_names,
            &mut selected_provider_name,
            &mut new_provider_name,
            &mut provider_editor_name,
            &mut provider_editor_alias,
            &mut provider_editor_enabled,
            &mut provider_editor_auth_token_env,
            &mut provider_editor_api_key_env,
            &mut provider_editor_endpoints,
            &mut action_upsert_provider_spec_remote,
            &mut action_delete_provider_spec_remote,
            &mut action_save_apply,
        );

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
}
