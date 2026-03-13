use super::*;

pub(in super::super) fn config_provider_endpoint_editor_from_spec(
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
    if ui
        .button(pick(lang, "新增 endpoint", "Add endpoint"))
        .clicked()
    {
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
pub(in super::super) fn render_config_v2_providers_section(
    ui: &mut egui::Ui,
    lang: Language,
    proxy_kind: ProxyModeKind,
    last_error: &mut Option<String>,
    last_info: &mut Option<String>,
    view: &mut crate::config::ServiceViewV2,
    selected_service: &str,
    provider_structure_control_plane_enabled: bool,
    provider_structure_edit_enabled: bool,
    attached_provider_specs: Option<&BTreeMap<String, PersistedProviderSpec>>,
    attached_station_specs: Option<&(
        BTreeMap<String, PersistedStationSpec>,
        BTreeMap<String, PersistedStationProviderRef>,
    )>,
    local_provider_spec_catalog: &BTreeMap<String, PersistedProviderSpec>,
    provider_display_names: &[String],
    selected_provider_name: &mut Option<String>,
    new_provider_name: &mut String,
    provider_editor_name: &mut Option<String>,
    provider_editor_alias: &mut String,
    provider_editor_enabled: &mut bool,
    provider_editor_auth_token_env: &mut String,
    provider_editor_api_key_env: &mut String,
    provider_editor_endpoints: &mut Vec<ConfigProviderEndpointEditorState>,
    action_upsert_provider_spec_remote: &mut Option<(String, PersistedProviderSpec)>,
    action_delete_provider_spec_remote: &mut Option<String>,
    action_save_apply: &mut bool,
) {
    ui.group(|ui| {
        ui.heading(pick(lang, "Providers", "Providers"));
        ui.label(pick(
            lang,
            "Provider 负责认证引用与 endpoint 集合；适合做快捷切换、故障切换和不同中转站的结构管理。这里不会显示明文密钥。",
            "Providers hold auth references plus endpoint sets; they are the right place for quick switching, failover, and relay structure management. Plaintext secrets are never shown here.",
        ));

        if provider_structure_control_plane_enabled || !matches!(proxy_kind, ProxyModeKind::Attached)
        {
            ui.columns(2, |cols| {
                cols[0].heading(pick(lang, "Provider 列表", "Provider list"));
                cols[0].add_space(4.0);
                cols[0].horizontal(|ui| {
                    ui.label(pick(lang, "新建 provider", "New provider"));
                    ui.add_sized(
                        [180.0, 22.0],
                        egui::TextEdit::singleline(new_provider_name)
                            .hint_text(pick(lang, "例如 right / backup", "e.g. right / backup")),
                    );
                    if ui
                        .add_enabled(
                            provider_structure_edit_enabled,
                            egui::Button::new(pick(lang, "新增", "Add")),
                        )
                        .clicked()
                    {
                        let name = new_provider_name.trim();
                        if name.is_empty() {
                            *last_error = Some(
                                pick(lang, "provider 名称不能为空。", "Provider name cannot be empty.")
                                    .to_string(),
                            );
                        } else if provider_structure_control_plane_enabled {
                            if attached_provider_specs
                                .is_some_and(|providers| providers.contains_key(name))
                            {
                                *last_error = Some(
                                    pick(lang, "provider 名称已存在。", "Provider name already exists.")
                                        .to_string(),
                                );
                            } else {
                                *action_upsert_provider_spec_remote = Some((
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
                                *selected_provider_name = Some(name.to_string());
                                *provider_editor_name = Some(name.to_string());
                                provider_editor_alias.clear();
                                *provider_editor_enabled = true;
                                provider_editor_auth_token_env.clear();
                                provider_editor_api_key_env.clear();
                                provider_editor_endpoints.clear();
                                new_provider_name.clear();
                            }
                        } else if view.providers.contains_key(name) {
                            *last_error = Some(
                                pick(lang, "provider 名称已存在。", "Provider name already exists.")
                                    .to_string(),
                            );
                        } else {
                            view.providers.insert(name.to_string(), ProviderConfigV2::default());
                            *selected_provider_name = Some(name.to_string());
                            *provider_editor_name = Some(name.to_string());
                            provider_editor_alias.clear();
                            *provider_editor_enabled = true;
                            provider_editor_auth_token_env.clear();
                            provider_editor_api_key_env.clear();
                            provider_editor_endpoints.clear();
                            new_provider_name.clear();
                            *last_info = Some(
                                pick(lang, "已新增 provider（待保存）。", "Provider added (save pending).")
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
                                lang,
                                "当前没有 provider。可以先新增一个空 provider，再补 endpoint 与 env 引用。",
                                "No providers yet. Add an empty provider first, then fill endpoints and env refs.",
                            ));
                        }
                        for name in provider_display_names {
                            let provider = if provider_structure_control_plane_enabled {
                                attached_provider_specs.and_then(|providers| providers.get(name))
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
                                *selected_provider_name = Some(name.clone());
                            }
                        }
                    });

                cols[1].heading(pick(lang, "Provider 详情", "Provider details"));
                cols[1].add_space(4.0);

                let Some(name) = selected_provider_name.clone() else {
                    cols[1].label(pick(lang, "未选择 provider。", "No provider selected."));
                    return;
                };

                let provider_snapshot = if provider_structure_control_plane_enabled {
                    let Some(provider) = attached_provider_specs
                        .and_then(|providers| providers.get(name.as_str()))
                        .cloned()
                    else {
                        cols[1].label(pick(
                            lang,
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
                            lang,
                            "provider 不存在（可能已被删除）。",
                            "Provider missing.",
                        ));
                        return;
                    };
                    provider
                };

                let referencing_stations = if provider_structure_control_plane_enabled {
                    attached_station_specs
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
                            lang,
                            "当前通过附着代理暴露的 provider 结构 API 直接管理远端 provider；明文密钥、tags、模型映射等高级字段仍不会在这里暴露。",
                            "This view manages the attached proxy through its provider structure API directly; plaintext secrets, tags, and model mappings are still not exposed here.",
                        )
                    } else {
                        pick(
                            lang,
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
                        egui::TextEdit::singleline(provider_editor_alias),
                    );
                    if ui.button(pick(lang, "清除", "Clear")).clicked() {
                        provider_editor_alias.clear();
                    }
                });

                cols[1].horizontal(|ui| {
                    ui.checkbox(provider_editor_enabled, pick(lang, "启用", "Enabled"));
                });

                cols[1].horizontal(|ui| {
                    ui.label("auth_token_env");
                    ui.add_sized(
                        [220.0, 22.0],
                        egui::TextEdit::singleline(provider_editor_auth_token_env),
                    );
                    if ui.button(pick(lang, "清除", "Clear")).clicked() {
                        provider_editor_auth_token_env.clear();
                    }
                });

                cols[1].horizontal(|ui| {
                    ui.label("api_key_env");
                    ui.add_sized(
                        [220.0, 22.0],
                        egui::TextEdit::singleline(provider_editor_api_key_env),
                    );
                    if ui.button(pick(lang, "清除", "Clear")).clicked() {
                        provider_editor_api_key_env.clear();
                    }
                });

                cols[1].add_space(8.0);
                cols[1].separator();
                cols[1].label(pick(lang, "Endpoints", "Endpoints"));
                render_config_provider_endpoint_editor(
                    &mut cols[1],
                    lang,
                    selected_service,
                    name.as_str(),
                    provider_editor_endpoints,
                );

                cols[1].add_space(8.0);
                cols[1].horizontal(|ui| {
                    if ui
                        .button(pick(
                            lang,
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
                            *provider_editor_enabled,
                            provider_editor_auth_token_env.as_str(),
                            provider_editor_api_key_env.as_str(),
                            provider_editor_endpoints,
                        ) {
                            Ok(provider_spec) => {
                                if provider_structure_control_plane_enabled {
                                    *action_upsert_provider_spec_remote =
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
                                    *action_save_apply = true;
                                }
                            }
                            Err(e) => {
                                *last_error = Some(e);
                            }
                        }
                    }

                    if ui
                        .button(pick(lang, "删除 provider", "Delete provider"))
                        .clicked()
                    {
                        if !provider_structure_control_plane_enabled
                            && !referencing_stations.is_empty()
                        {
                            *last_error = Some(format!(
                                "{}: {}",
                                pick(
                                    lang,
                                    "仍有 station 引用了该 provider，不能删除",
                                    "Stations still reference this provider; delete is blocked",
                                ),
                                referencing_stations.join(", ")
                            ));
                        } else if provider_structure_control_plane_enabled {
                            *action_delete_provider_spec_remote = Some(name.clone());
                        } else {
                            view.providers.remove(name.as_str());
                            *selected_provider_name = view.providers.keys().next().cloned();
                            *last_info = Some(
                                pick(
                                    lang,
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
                    lang,
                    "当前附着目标还没有暴露 provider 结构 API；这里保持只读，避免误导为会写回本机文件。需要查看高级字段时请使用 Raw 视图。",
                    "This attached target does not expose provider structure APIs yet; this section stays read-only to avoid implying local-file writes. Use Raw view for advanced fields.",
                ),
            );
        }
    });
}
