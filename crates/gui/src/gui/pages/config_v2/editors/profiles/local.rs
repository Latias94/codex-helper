use super::shared::{build_profile_card_item, render_profile_card_list};
use super::*;

pub(in super::super::super) struct LocalProfilesSectionArgs<'a> {
    pub lang: Language,
    pub selected_service: &'a str,
    pub view: &'a mut crate::config::ServiceViewV2,
    pub station_names: &'a [String],
    pub selected_profile_name: &'a mut Option<String>,
    pub new_profile_name: &'a mut String,
    pub profile_info: &'a mut Option<String>,
    pub profile_error: &'a mut Option<String>,
    pub action_save_apply: &'a mut bool,
    pub configured_active_name: Option<&'a str>,
    pub effective_active_name: Option<&'a str>,
    pub preview_station_specs: Option<&'a BTreeMap<String, PersistedStationSpec>>,
    pub preview_provider_catalog: Option<&'a BTreeMap<String, PersistedStationProviderRef>>,
    pub preview_runtime_station_catalog: Option<&'a BTreeMap<String, StationOption>>,
}

pub(in super::super::super) fn render_config_v2_profiles_local(
    ui: &mut egui::Ui,
    args: LocalProfilesSectionArgs<'_>,
) {
    let LocalProfilesSectionArgs {
        lang,
        selected_service,
        view,
        station_names,
        selected_profile_name,
        new_profile_name,
        profile_info,
        profile_error,
        action_save_apply,
        configured_active_name,
        effective_active_name,
        preview_station_specs,
        preview_provider_catalog,
        preview_runtime_station_catalog,
    } = args;

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
            } else if view.profiles.contains_key(name) {
                *profile_error = Some(
                    pick(lang, "profile 名称已存在。", "Profile name already exists.").to_string(),
                );
            } else {
                view.profiles.insert(
                    name.to_string(),
                    crate::config::ServiceControlProfile::default(),
                );
                if view.default_profile.is_none() {
                    view.default_profile = Some(name.to_string());
                }
                *selected_profile_name = Some(name.to_string());
                new_profile_name.clear();
                *profile_info = Some(
                    pick(
                        lang,
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
        cols[0].label(pick(lang, "策略预设", "Strategy presets"));
        cols[0].small(pick(
            lang,
            "左侧更适合快速挑选日常 profile；右侧再做细节编辑。",
            "Use the left deck for quick daily selection, then edit details on the right.",
        ));
        cols[0].add_space(4.0);
        let names = view.profiles.keys().cloned().collect::<Vec<_>>();
        let cards = names
            .iter()
            .filter_map(|name| {
                view.profiles.get(name).map(|profile| {
                    build_profile_card_item(
                        name.as_str(),
                        profile,
                        view.default_profile.as_deref() == Some(name.as_str()),
                        selected_profile_name.as_deref() == Some(name.as_str()),
                    )
                })
            })
            .collect::<Vec<_>>();
        render_profile_card_list(
            &mut cols[0],
            lang,
            "config_v2_profiles_scroll",
            pick(lang, "(当前没有 profile)", "(no profiles yet)"),
            &cards,
            |name| {
                *selected_profile_name = Some(name.to_string());
            },
        );

        cols[1].label(pick(lang, "Profile 详情", "Profile details"));
        cols[1].add_space(4.0);

        let Some(profile_name) = selected_profile_name.clone() else {
            cols[1].label(pick(lang, "未选择 profile。", "No profile selected."));
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
            cols[1].label(pick(lang, "profile 不存在（可能已被删除）。", "Profile missing."));
            return;
        };

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
                view.default_profile = Some(profile_name.clone());
                *profile_info = Some(
                    pick(
                        lang,
                        "已更新 default_profile（待保存）。",
                        "default_profile updated (save pending).",
                    )
                    .to_string(),
                );
            }
            if ui
                .button(pick(lang, "清除 default_profile", "Clear default_profile"))
                .clicked()
                && is_default
            {
                view.default_profile = None;
                *profile_info = Some(
                    pick(
                        lang,
                        "已清除 default_profile（待保存）。",
                        "default_profile cleared (save pending).",
                    )
                    .to_string(),
                );
            }
            if ui.button(pick(lang, "删除 profile", "Delete profile")).clicked() {
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
            ui.label(pick(lang, "station", "station"));
            egui::ComboBox::from_id_salt(format!(
                "config_v2_profile_station_{selected_service}_{profile_name}"
            ))
            .selected_text(
                station
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "<自动>", "<auto>")),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut station, None, pick(lang, "<自动>", "<auto>"));
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
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
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
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
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
            if ui.button(pick(lang, "清除", "Clear")).clicked() {
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
            fast_mode: declared_profile.service_tier.as_deref() == Some("priority"),
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
            configured_active_name,
            effective_active_name,
            preview_station_specs,
            preview_provider_catalog,
            preview_runtime_station_catalog,
        );
        render_profile_route_preview(&mut cols[1], lang, &preview_profile, &profile_preview);

        if delete_selected {
            view.profiles.remove(profile_name.as_str());
            if view.default_profile.as_deref() == Some(profile_name.as_str()) {
                view.default_profile = None;
            }
            *selected_profile_name = view
                .default_profile
                .clone()
                .or_else(|| view.profiles.keys().next().cloned());
            *profile_info = Some(
                pick(lang, "已删除 profile（待保存）。", "Profile deleted (save pending).")
                    .to_string(),
            );
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
    {
        *action_save_apply = true;
    }
}
