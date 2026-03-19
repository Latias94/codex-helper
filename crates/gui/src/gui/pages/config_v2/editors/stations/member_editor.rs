use super::*;

pub(crate) fn config_station_member_editor_from_member(
    member: &GroupMemberRefV2,
) -> StationMemberEditorState {
    StationMemberEditorState {
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
    members: &[StationMemberEditorState],
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
    members: &mut Vec<StationMemberEditorState>,
) {
    let default_provider = provider_catalog.keys().next().cloned().unwrap_or_default();

    if ui.button(pick(lang, "新增成员", "Add member")).clicked() {
        members.push(StationMemberEditorState {
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
    members: &[StationMemberEditorState],
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
