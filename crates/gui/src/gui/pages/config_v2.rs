use super::*;

mod actions;
mod editors;
mod state;

use actions::*;
use editors::*;
use state::*;

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

    let mut actions = ConfigV2PendingActions::default();
    let mut draft = ConfigV2EditorDraft::from_view(&ctx.view.config);
    if station_structure_control_plane_enabled {
        if let Some((station_specs, _)) = attached_station_specs.as_ref() {
            draft.sync_station_editor_from_specs(selected_name.as_deref(), station_specs);
        }
    } else if station_control_plane_enabled {
        draft.sync_station_editor_from_runtime(
            selected_name.as_deref(),
            &station_control_plane_catalog,
        );
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
    if profile_control_plane_enabled {
        draft.sync_selected_profile_name_remote(
            &profile_control_plane_catalog,
            profile_control_plane_default.as_deref(),
        );
        draft.sync_profile_editor_from_remote(&profile_control_plane_catalog);
    } else {
        draft.sync_selected_profile_name_local(&profile_names, default_profile.as_deref());
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
            && !station_structure_control_plane_enabled
            && !station_control_plane_enabled
        {
            draft.sync_station_editor_from_specs(
                selected_name.as_deref(),
                &local_station_spec_catalog,
            );
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
        draft.sync_selected_provider_name(&provider_display_names);
        if provider_structure_control_plane_enabled {
            if let Some(provider_specs) = attached_provider_specs.as_ref() {
                draft.sync_provider_editor_from_specs(provider_specs);
            }
        } else if !matches!(ctx.proxy.kind(), ProxyModeKind::Attached) {
            draft.sync_provider_editor_from_specs(&local_provider_spec_catalog);
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
                action_set_active: &mut actions.set_active,
                action_clear_active: &mut actions.clear_active,
                action_set_active_remote: &mut actions.set_active_remote,
                action_save_apply: &mut actions.save_apply,
                action_save_apply_remote: &mut actions.save_apply_remote,
                action_upsert_station_spec_remote: &mut actions.upsert_station_spec_remote,
                action_delete_station_spec_remote: &mut actions.delete_station_spec_remote,
                action_probe_selected: &mut actions.probe_selected,
                action_health_start: &mut actions.health_start,
                action_health_cancel: &mut actions.health_cancel,
                new_station_name: &mut draft.new_station_name,
                station_editor_name: &mut draft.station_editor_name,
                station_editor_alias: &mut draft.station_editor_alias,
                station_editor_enabled: &mut draft.station_editor_enabled,
                station_editor_level: &mut draft.station_editor_level,
                station_editor_members: &mut draft.station_editor_members,
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
            &mut draft.selected_provider_name,
            &mut draft.new_provider_name,
            &mut draft.provider_editor_name,
            &mut draft.provider_editor_alias,
            &mut draft.provider_editor_enabled,
            &mut draft.provider_editor_auth_token_env,
            &mut draft.provider_editor_api_key_env,
            &mut draft.provider_editor_endpoints,
            &mut actions.upsert_provider_spec_remote,
            &mut actions.delete_provider_spec_remote,
            &mut actions.save_apply,
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
                    &mut draft.selected_profile_name,
                    &mut draft.new_profile_name,
                    &mut draft.profile_editor_name,
                    &mut draft.profile_editor_extends,
                    &mut draft.profile_editor_station,
                    &mut draft.profile_editor_model,
                    &mut draft.profile_editor_reasoning_effort,
                    &mut draft.profile_editor_service_tier,
                    &mut draft.profile_error,
                    &mut actions.profile_upsert_remote,
                    &mut actions.profile_delete_remote,
                    &mut actions.profile_set_persisted_default_remote,
                    matches!(ctx.proxy.kind(), ProxyModeKind::Attached),
                    station_control_plane_enabled,
                    station_control_plane_configured_active.as_deref(),
                    station_control_plane_effective_active.as_deref(),
                    preview_station_specs,
                    preview_provider_catalog,
                    preview_runtime_station_catalog,
                );
            } else {
                render_config_v2_profiles_local(
                    ui,
                    LocalProfilesSectionArgs {
                        lang: ctx.lang,
                        selected_service,
                        view,
                        station_names: &station_names,
                        selected_profile_name: &mut draft.selected_profile_name,
                        new_profile_name: &mut draft.new_profile_name,
                        profile_info: &mut draft.profile_info,
                        profile_error: &mut draft.profile_error,
                        action_save_apply: &mut actions.save_apply,
                        configured_active_name: configured_active_name.as_deref(),
                        effective_active_name: effective_active_name.as_deref(),
                        preview_station_specs,
                        preview_provider_catalog,
                        preview_runtime_station_catalog,
                    },
                );
            }
        });
    }

    let (profile_info, profile_error) = draft.persist_into_view(&mut ctx.view.config);
    if let Some(message) = profile_info {
        *ctx.last_info = Some(message);
    }
    if let Some(message) = profile_error {
        *ctx.last_error = Some(message);
    }
    actions.apply(ctx);
}
