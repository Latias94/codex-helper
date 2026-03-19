use super::member_editor::{
    build_station_spec_from_config_editor, render_config_station_member_editor,
    render_config_station_provider_summary,
};
use super::*;

pub(crate) struct StationsSectionArgs<'a> {
    pub lang: Language,
    pub proxy_kind: ProxyModeKind,
    pub last_error: &'a mut Option<String>,
    pub last_info: &'a mut Option<String>,
    pub view: &'a mut crate::config::ServiceViewV2,
    pub selected_service: &'a str,
    pub schema_version: u32,
    pub station_display_names: &'a [String],
    pub selected_name: &'a mut Option<String>,
    pub station_control_plane_enabled: bool,
    pub station_structure_control_plane_enabled: bool,
    pub station_structure_edit_enabled: bool,
    pub station_control_plane_catalog: &'a BTreeMap<String, StationOption>,
    pub configured_active_name: Option<String>,
    pub effective_active_name: Option<String>,
    pub station_default_profile: Option<String>,
    pub attached_station_specs: Option<&'a (
        BTreeMap<String, PersistedStationSpec>,
        BTreeMap<String, PersistedStationProviderRef>,
    )>,
    pub local_station_spec_catalog: &'a BTreeMap<String, PersistedStationSpec>,
    pub local_provider_ref_catalog: &'a BTreeMap<String, PersistedStationProviderRef>,
    pub provider_catalog: &'a BTreeMap<String, ProviderConfigV2>,
    pub profile_catalog: &'a BTreeMap<String, crate::config::ServiceControlProfile>,
    pub runtime_service: Option<&'a str>,
    pub supports_v1: bool,
    pub cfg_health: Option<&'a StationHealth>,
    pub hc_status: Option<&'a HealthCheckStatus>,
    pub action_set_active: &'a mut Option<String>,
    pub action_clear_active: &'a mut bool,
    pub action_set_active_remote: &'a mut Option<Option<String>>,
    pub action_save_apply: &'a mut bool,
    pub action_save_apply_remote: &'a mut Option<(String, bool, u8)>,
    pub action_upsert_station_spec_remote: &'a mut Option<(String, PersistedStationSpec)>,
    pub action_delete_station_spec_remote: &'a mut Option<String>,
    pub action_probe_selected: &'a mut Option<String>,
    pub action_health_start: &'a mut Option<(bool, Vec<String>)>,
    pub action_health_cancel: &'a mut Option<(bool, Vec<String>)>,
    pub new_station_name: &'a mut String,
    pub station_editor_name: &'a mut Option<String>,
    pub station_editor_alias: &'a mut String,
    pub station_editor_enabled: &'a mut bool,
    pub station_editor_level: &'a mut u8,
    pub station_editor_members: &'a mut Vec<StationMemberEditorState>,
}

pub(crate) fn render_config_v2_stations_section(ui: &mut egui::Ui, args: StationsSectionArgs<'_>) {
    let StationsSectionArgs {
        lang,
        proxy_kind,
        last_error,
        last_info,
        view,
        selected_service,
        schema_version,
        station_display_names,
        selected_name,
        station_control_plane_enabled,
        station_structure_control_plane_enabled,
        station_structure_edit_enabled,
        station_control_plane_catalog,
        configured_active_name,
        effective_active_name,
        station_default_profile,
        attached_station_specs,
        local_station_spec_catalog,
        local_provider_ref_catalog,
        provider_catalog,
        profile_catalog,
        runtime_service,
        supports_v1,
        cfg_health,
        hc_status,
        action_set_active,
        action_clear_active,
        action_set_active_remote,
        action_save_apply,
        action_save_apply_remote,
        action_upsert_station_spec_remote,
        action_delete_station_spec_remote,
        action_probe_selected,
        action_health_start,
        action_health_cancel,
        new_station_name,
        station_editor_name,
        station_editor_alias,
        station_editor_enabled,
        station_editor_level,
        station_editor_members,
    } = args;
    ui.columns(2, |cols| {
        cols[0].heading(pick(lang, "站点列表", "Stations"));
        cols[0].add_space(4.0);
        cols[0].horizontal(|ui| {
            ui.label(pick(lang, "新建 station", "New station"));
            ui.add_sized(
                [180.0, 22.0],
                egui::TextEdit::singleline(new_station_name).hint_text(pick(
                    lang,
                    "例如 primary / backup",
                    "e.g. primary / backup",
                )),
            );
            if ui
                .add_enabled(
                    station_structure_edit_enabled,
                    egui::Button::new(pick(lang, "新增", "Add")),
                )
                .clicked()
            {
                let name = new_station_name.trim();
                if name.is_empty() {
                    *last_error = Some(
                        pick(lang, "station 名称不能为空。", "Station name cannot be empty.")
                            .to_string(),
                    );
                } else if station_structure_control_plane_enabled {
                    if attached_station_specs.is_some_and(|specs| specs.0.contains_key(name)) {
                        *last_error = Some(
                            pick(lang, "station 名称已存在。", "Station name already exists.")
                                .to_string(),
                        );
                    } else {
                        *action_upsert_station_spec_remote = Some((
                            name.to_string(),
                            PersistedStationSpec {
                                name: name.to_string(),
                                alias: None,
                                enabled: true,
                                level: 1,
                                members: Vec::new(),
                            },
                        ));
                        *selected_name = Some(name.to_string());
                        *station_editor_name = Some(name.to_string());
                        station_editor_alias.clear();
                        *station_editor_enabled = true;
                        *station_editor_level = 1;
                        station_editor_members.clear();
                        new_station_name.clear();
                    }
                } else if view.groups.contains_key(name) {
                    *last_error = Some(
                        pick(lang, "station 名称已存在。", "Station name already exists.")
                            .to_string(),
                    );
                } else {
                    view.groups.insert(name.to_string(), GroupConfigV2::default());
                    *selected_name = Some(name.to_string());
                    *station_editor_name = Some(name.to_string());
                    new_station_name.clear();
                    *last_info = Some(
                        pick(lang, "已新增 station（待保存）。", "Station added (save pending).")
                            .to_string(),
                    );
                }
            }
        });
        if !station_structure_edit_enabled {
            cols[0].small(pick(
                lang,
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
                        lang,
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
                        *selected_name = Some(name.clone());
                    }
                }
            });

        cols[1].heading(pick(lang, "站点详情", "Station Details"));
        cols[1].add_space(4.0);

        let Some(name) = selected_name.clone() else {
            cols[1].label(pick(lang, "未选择站点。", "No station selected."));
            return;
        };

        let active_label = if configured_active_name.as_deref() == Some(name.as_str()) {
            pick(lang, "是", "yes")
        } else {
            pick(lang, "否", "no")
        };
        let effective_label = effective_active_name
            .clone()
            .unwrap_or_else(|| pick(lang, "(无)", "(none)").to_string());

        cols[1].label(format!("schema: v{schema_version}"));
        cols[1].label(format!("active_station: {active_label}"));
        cols[1].label(format!(
            "{}: {effective_label}",
            pick(lang, "生效站点", "Effective station")
        ));
        cols[1].label(format!(
            "default_profile: {}",
            station_default_profile.as_deref().unwrap_or("-")
        ));
        cols[1].add_space(6.0);

        if station_structure_control_plane_enabled || !matches!(proxy_kind, ProxyModeKind::Attached)
        {
            let (station_snapshot, provider_ref_catalog) = if station_structure_control_plane_enabled {
                let Some((station_specs, provider_specs)) = attached_station_specs else {
                    cols[1].label(pick(
                        lang,
                        "远端 station 结构视图不可用。",
                        "Remote station structure view is unavailable.",
                    ));
                    return;
                };
                let Some(station_snapshot) = station_specs.get(&name).cloned() else {
                    cols[1].label(pick(
                        lang,
                        "远端 station 不存在（可能已被删除）。",
                        "Remote station missing.",
                    ));
                    return;
                };
                (station_snapshot, provider_specs)
            } else {
                let Some(station_snapshot) = local_station_spec_catalog.get(&name).cloned() else {
                    cols[1].label(pick(
                        lang,
                        "站点不存在（可能已被删除）。",
                        "Station missing.",
                    ));
                    return;
                };
                (station_snapshot, local_provider_ref_catalog)
            };

            let referencing_profiles = profile_catalog
                .iter()
                .filter_map(|(profile_name, profile)| {
                    (profile.station.as_deref() == Some(name.as_str())).then_some(profile_name.clone())
                })
                .collect::<Vec<_>>();

            cols[1].colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                if station_structure_control_plane_enabled {
                    pick(
                        lang,
                        "当前通过附着代理暴露的 station 结构 API 直接管理远端配置；provider 密钥仍不会通过这里暴露。",
                        "This view manages the attached proxy through its station structure API directly; provider secrets are still not exposed here.",
                    )
                } else {
                    pick(
                        lang,
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
                    egui::TextEdit::singleline(station_editor_alias),
                );
                if ui.button(pick(lang, "清除", "Clear")).clicked() {
                    station_editor_alias.clear();
                }
            });

            cols[1].horizontal(|ui| {
                ui.checkbox(station_editor_enabled, pick(lang, "启用", "Enabled"));
                ui.label(pick(lang, "等级", "Level"));
                ui.add(egui::DragValue::new(station_editor_level).range(1..=10));
            });

            cols[1].add_space(8.0);
            cols[1].separator();
            cols[1].label(pick(lang, "成员引用", "Members"));
            render_config_station_member_editor(
                &mut cols[1],
                lang,
                selected_service,
                provider_ref_catalog,
                station_editor_members,
            );

            cols[1].add_space(8.0);
            cols[1].separator();
            cols[1].label(pick(lang, "可用 Provider", "Available Providers"));
            render_config_station_provider_summary(
                &mut cols[1],
                lang,
                provider_ref_catalog,
                station_editor_members,
            );

            cols[1].add_space(8.0);
            cols[1].horizontal(|ui| {
                if ui
                    .button(pick(lang, "设为 active_station", "Set active_station"))
                    .clicked()
                {
                    if station_control_plane_enabled {
                        *action_set_active_remote = Some(Some(name.clone()));
                    } else {
                        *action_set_active = Some(name.clone());
                    }
                }

                if ui
                    .button(pick(lang, "清除 active_station", "Clear active_station"))
                    .clicked()
                {
                    if station_control_plane_enabled {
                        *action_set_active_remote = Some(None);
                    } else {
                        *action_clear_active = true;
                    }
                }

                if ui.button(pick(lang, "删除 station", "Delete station")).clicked() {
                    if !referencing_profiles.is_empty() {
                        *last_error = Some(format!(
                            "{}: {}",
                            pick(
                                lang,
                                "仍有 profile 引用了该 station，不能删除",
                                "Profiles still reference this station; delete is blocked",
                            ),
                            referencing_profiles.join(", ")
                        ));
                    } else if station_structure_control_plane_enabled {
                        *action_delete_station_spec_remote = Some(name.clone());
                    } else {
                        view.groups.remove(name.as_str());
                        if view.active_group.as_deref() == Some(name.as_str()) {
                            view.active_group = None;
                        }
                        *selected_name = view.groups.keys().next().cloned();
                        *last_info = Some(
                            pick(lang, "已删除 station（待保存）。", "Station deleted (save pending).")
                                .to_string(),
                        );
                    }
                }

                if ui
                    .button(pick(
                        lang,
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
                        *station_editor_enabled,
                        *station_editor_level,
                        station_editor_members,
                    ) {
                        Ok(station_spec) => {
                            if station_structure_control_plane_enabled {
                                *action_upsert_station_spec_remote = Some((name.clone(), station_spec));
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
                                *action_save_apply = true;
                            }
                        }
                        Err(e) => {
                            *last_error = Some(e);
                        }
                    }
                }
            });
        } else {
            let Some(station_snapshot) = station_control_plane_catalog.get(&name).cloned() else {
                cols[1].label(pick(
                    lang,
                    "远端 station 不存在（可能已被删除）。",
                    "Remote station missing.",
                ));
                return;
            };
            cols[1].colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                pick(
                    lang,
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
                pick(lang, "配置启用", "Configured enabled"),
                station_snapshot.configured_enabled
            ));
            cols[1].label(format!(
                "{}: {}",
                pick(lang, "配置等级", "Configured level"),
                station_snapshot.configured_level
            ));
            cols[1].label(format!(
                "{}: {:?}",
                pick(lang, "运行状态", "Runtime state"),
                station_snapshot.runtime_state
            ));
            cols[1].label(format!(
                "{}: {}",
                pick(lang, "运行时 enabled 覆盖", "Runtime enabled override"),
                station_snapshot
                    .runtime_enabled_override
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string())
            ));
            cols[1].label(format!(
                "{}: {}",
                pick(lang, "运行时 level 覆盖", "Runtime level override"),
                station_snapshot
                    .runtime_level_override
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string())
            ));
            cols[1].label(format!(
                "{}: {:?}",
                pick(lang, "模型目录", "Model catalog"),
                station_snapshot.capabilities.model_catalog_kind
            ));
            cols[1].label(format!(
                "{}: {}",
                pick(lang, "支持 service tier", "Supports service tier"),
                capability_support_label(lang, station_snapshot.capabilities.supports_service_tier)
            ));
            cols[1].label(format!(
                "{}: {}",
                pick(lang, "支持 reasoning", "Supports reasoning"),
                capability_support_label(
                    lang,
                    station_snapshot.capabilities.supports_reasoning_effort,
                )
            ));
            if !station_snapshot.capabilities.supported_models.is_empty() {
                cols[1].small(format!(
                    "{}: {}",
                    pick(lang, "支持模型", "Supported models"),
                    station_snapshot.capabilities.supported_models.join(", ")
                ));
            }
            cols[1].add_space(6.0);
            cols[1].horizontal(|ui| {
                ui.checkbox(station_editor_enabled, pick(lang, "启用", "Enabled"));
                ui.label(pick(lang, "等级", "Level"));
                ui.add(egui::DragValue::new(station_editor_level).range(1..=10));
            });
        }

        cols[1].add_space(8.0);
        cols[1].separator();
        cols[1].label(pick(lang, "健康检查", "Health check"));
        if runtime_service.is_none() {
            cols[1].label(pick(
                lang,
                "代理未运行/未附着，无法执行健康检查。",
                "Proxy is not running/attached; health check disabled.",
            ));
        } else if !supports_v1 {
            cols[1].label(pick(
                lang,
                "附着代理未启用 API v1：健康检查不可用。",
                "Attached proxy has no API v1: health check disabled.",
            ));
        } else if runtime_service != Some(selected_service) {
            cols[1].label(pick(
                lang,
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
                cols[1].label(pick(lang, "(无状态)", "(no status)"));
            }

            cols[1].horizontal(|ui| {
                if ui.button(pick(lang, "探测当前", "Probe selected")).clicked() {
                    *action_probe_selected = Some(name.clone());
                }
                if ui.button(pick(lang, "取消当前", "Cancel selected")).clicked() {
                    *action_health_cancel = Some((false, vec![name.clone()]));
                }
                if ui.button(pick(lang, "检查全部", "Check all")).clicked() {
                    *action_health_start = Some((true, Vec::new()));
                }
                if ui.button(pick(lang, "取消全部", "Cancel all")).clicked() {
                    *action_health_cancel = Some((true, Vec::new()));
                }
            });

            if let Some(h) = cfg_health.as_ref() {
                cols[1].add_space(6.0);
                cols[1].label(format!(
                    "{}: {}  upstreams={}",
                    pick(lang, "最近检查", "Last checked"),
                    h.checked_at_ms,
                    h.upstreams.len()
                ));
            }
        }

        if matches!(proxy_kind, ProxyModeKind::Attached) && !station_structure_control_plane_enabled
        {
            cols[1].add_space(6.0);
            cols[1].horizontal(|ui| {
                if ui
                    .button(pick(lang, "设为 active_station", "Set active_station"))
                    .clicked()
                {
                    if station_control_plane_enabled {
                        *action_set_active_remote = Some(Some(name.clone()));
                    } else {
                        *action_set_active = Some(name.clone());
                    }
                }

                if ui
                    .button(pick(lang, "清除 active_station", "Clear active_station"))
                    .clicked()
                {
                    if station_control_plane_enabled {
                        *action_set_active_remote = Some(None);
                    } else {
                        *action_clear_active = true;
                    }
                }

                if ui
                    .button(pick(
                        lang,
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
                        *action_save_apply_remote = Some((
                            name.clone(),
                            *station_editor_enabled,
                            (*station_editor_level).clamp(1, 10),
                        ));
                    } else {
                        *action_save_apply = true;
                    }
                }
            });
        }
    });
}
