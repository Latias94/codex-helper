use super::*;

#[allow(clippy::too_many_arguments)]
pub(in super::super) fn render_config_v2_profiles_control_plane(
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
                    pick(
                        lang,
                        "profile 名称不能为空。",
                        "Profile name cannot be empty.",
                    )
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
