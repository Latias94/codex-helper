use super::endpoints::{
    build_provider_spec_from_config_editor, merge_provider_spec_into_provider_config,
    render_config_provider_endpoint_editor,
};
use super::helpers::{
    provider_spec_catalog, provider_spec_snapshot, provider_station_refs,
    render_provider_detail_badge, render_provider_overview_cards, render_provider_summary_card,
    sync_provider_editor_from_selected,
};
use super::shared::{build_provider_card_item, render_provider_card_list};
use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_config_v2_providers_section(
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
    provider_editor_endpoints: &mut Vec<ProviderEndpointEditorState>,
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
            let station_refs = provider_station_refs(
                view,
                attached_station_specs,
                provider_structure_control_plane_enabled,
            );
            let provider_cards = provider_display_names
                .iter()
                .filter_map(|name| {
                    let provider = provider_spec_snapshot(
                        name.as_str(),
                        provider_structure_control_plane_enabled,
                        attached_provider_specs,
                        local_provider_spec_catalog,
                    )?;
                    Some(build_provider_card_item(
                        lang,
                        provider,
                        station_refs
                            .get(name.as_str())
                            .map(Vec::as_slice)
                            .unwrap_or(&[]),
                        selected_provider_name.as_deref() == Some(name.as_str()),
                    ))
                })
                .collect::<Vec<_>>();
            let enabled_provider_count = provider_cards.iter().filter(|item| item.enabled).count();
            let fallback_ready_count =
                provider_cards.iter().filter(|item| item.failover_ready).count();
            let auth_ready_count = provider_cards
                .iter()
                .filter(|item| item.has_auth_token_env || item.has_api_key_env)
                .count();

            ui.columns(4, |cols| {
                render_provider_summary_card(
                    &mut cols[0],
                    pick(lang, "Providers", "Providers"),
                    provider_cards.len().to_string(),
                    pick(lang, "当前来源总数", "Relay sources in this service"),
                );
                render_provider_summary_card(
                    &mut cols[1],
                    pick(lang, "Enabled", "Enabled"),
                    enabled_provider_count.to_string(),
                    pick(lang, "当前可参与路由", "Currently available for routing"),
                );
                render_provider_summary_card(
                    &mut cols[2],
                    pick(lang, "Auth Ready", "Auth Ready"),
                    auth_ready_count.to_string(),
                    pick(
                        lang,
                        "已声明 env 引用的 provider",
                        "Providers with env references declared",
                    ),
                );
                render_provider_summary_card(
                    &mut cols[3],
                    pick(lang, "Fallback Ready", "Fallback Ready"),
                    fallback_ready_count.to_string(),
                    pick(
                        lang,
                        "启用多个 endpoint，适合主备切换",
                        "Multiple enabled endpoints, suitable for failover",
                    ),
                );
            });
            ui.add_space(8.0);

            ui.columns(2, |cols| {
                cols[0].label(pick(lang, "来源卡片", "Provider deck"));
                cols[0].small(pick(
                    lang,
                    "左侧更适合快速识别主用/备用来源、认证准备度和被哪些 station 引用；右侧再做结构编辑。",
                    "Use the left deck to scan primary/backup candidates, auth readiness, and station usage; edit structure on the right.",
                ));
                cols[0].add_space(4.0);
                cols[0].group(|ui| {
                    ui.horizontal(|ui| {
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
                            handle_add_provider(
                                lang,
                                view,
                                new_provider_name,
                                selected_provider_name,
                                provider_editor_name,
                                provider_editor_alias,
                                provider_editor_enabled,
                                provider_editor_auth_token_env,
                                provider_editor_api_key_env,
                                provider_editor_endpoints,
                                provider_structure_control_plane_enabled,
                                attached_provider_specs,
                                action_upsert_provider_spec_remote,
                                last_error,
                                last_info,
                            );
                        }
                    });
                });
                cols[0].add_space(6.0);
                render_provider_card_list(
                    &mut cols[0],
                    lang,
                    "config_v2_providers_scroll",
                    pick(
                        lang,
                        "当前没有 provider。可以先新增一个空 provider，再补 endpoint 与 env 引用。",
                        "No providers yet. Add an empty provider first, then fill endpoints and env refs.",
                    ),
                    &provider_cards,
                    |name| {
                        *selected_provider_name = Some(name.to_string());
                    },
                );

                if let Some(provider_specs) = provider_spec_catalog(
                    provider_structure_control_plane_enabled,
                    attached_provider_specs,
                    local_provider_spec_catalog,
                ) {
                    sync_provider_editor_from_selected(
                        selected_provider_name,
                        provider_editor_name,
                        provider_editor_alias,
                        provider_editor_enabled,
                        provider_editor_auth_token_env,
                        provider_editor_api_key_env,
                        provider_editor_endpoints,
                        provider_specs,
                    );
                }

                cols[1].label(pick(lang, "Provider 工作台", "Provider workbench"));
                cols[1].add_space(4.0);

                let Some(name) = selected_provider_name.clone() else {
                    cols[1].label(pick(lang, "未选择 provider。", "No provider selected."));
                    return;
                };

                let Some(provider_snapshot) = provider_spec_snapshot(
                    name.as_str(),
                    provider_structure_control_plane_enabled,
                    attached_provider_specs,
                    local_provider_spec_catalog,
                )
                .cloned()
                else {
                    cols[1].label(if provider_structure_control_plane_enabled {
                        pick(
                            lang,
                            "远端 provider 不存在（可能已被删除）。",
                            "Remote provider missing.",
                        )
                    } else {
                        pick(
                            lang,
                            "provider 不存在（可能已被删除）。",
                            "Provider missing.",
                        )
                    });
                    return;
                };
                let referencing_stations =
                    station_refs.get(name.as_str()).cloned().unwrap_or_default();
                let provider_card =
                    build_provider_card_item(lang, &provider_snapshot, &referencing_stations, true);

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
                cols[1].horizontal_wrapped(|ui| {
                    ui.heading(name.as_str());
                    if !provider_card.alias.is_empty() {
                        render_provider_detail_badge(
                            ui,
                            format!("alias {}", provider_card.alias),
                            egui::Color32::from_rgb(76, 114, 176),
                        );
                    }
                    render_provider_detail_badge(
                        ui,
                        if provider_card.enabled {
                            pick(lang, "enabled", "enabled")
                        } else {
                            pick(lang, "off", "off")
                        },
                        if provider_card.enabled {
                            egui::Color32::from_rgb(86, 122, 62)
                        } else {
                            egui::Color32::from_rgb(150, 150, 150)
                        },
                    );
                    if provider_card.failover_ready {
                        render_provider_detail_badge(
                            ui,
                            pick(lang, "fallback-ready", "fallback-ready"),
                            egui::Color32::from_rgb(176, 122, 76),
                        );
                    }
                });
                cols[1].small(provider_card.station_summary.as_str());
                cols[1].small(format!(
                    "{} · {}",
                    provider_card.auth_summary, provider_card.endpoint_summary
                ));
                cols[1].add_space(6.0);
                render_provider_overview_cards(&mut cols[1], lang, &provider_card);

                cols[1].add_space(8.0);
                cols[1].separator();
                cols[1].label(pick(lang, "Identity", "Identity"));
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

                cols[1].add_space(8.0);
                cols[1].separator();
                cols[1].label(pick(lang, "Auth refs", "Auth refs"));
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
                cols[1].small(pick(
                    lang,
                    "这里维护这个 provider 的来源集合。多个启用 endpoint 更适合后续的故障切换与备用链路。",
                    "Maintain the upstream set here. Multiple enabled endpoints are a better base for future failover and backup paths.",
                ));
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

#[allow(clippy::too_many_arguments)]
fn handle_add_provider(
    lang: Language,
    view: &mut crate::config::ServiceViewV2,
    new_provider_name: &mut String,
    selected_provider_name: &mut Option<String>,
    provider_editor_name: &mut Option<String>,
    provider_editor_alias: &mut String,
    provider_editor_enabled: &mut bool,
    provider_editor_auth_token_env: &mut String,
    provider_editor_api_key_env: &mut String,
    provider_editor_endpoints: &mut Vec<ProviderEndpointEditorState>,
    provider_structure_control_plane_enabled: bool,
    attached_provider_specs: Option<&BTreeMap<String, PersistedProviderSpec>>,
    action_upsert_provider_spec_remote: &mut Option<(String, PersistedProviderSpec)>,
    last_error: &mut Option<String>,
    last_info: &mut Option<String>,
) {
    let name = new_provider_name.trim();
    if name.is_empty() {
        *last_error = Some(
            pick(
                lang,
                "provider 名称不能为空。",
                "Provider name cannot be empty.",
            )
            .to_string(),
        );
        return;
    }

    if provider_structure_control_plane_enabled {
        if attached_provider_specs.is_some_and(|providers| providers.contains_key(name)) {
            *last_error = Some(
                pick(
                    lang,
                    "provider 名称已存在。",
                    "Provider name already exists.",
                )
                .to_string(),
            );
            return;
        }

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
    } else if view.providers.contains_key(name) {
        *last_error = Some(
            pick(
                lang,
                "provider 名称已存在。",
                "Provider name already exists.",
            )
            .to_string(),
        );
        return;
    } else {
        view.providers
            .insert(name.to_string(), ProviderConfigV2::default());
        *last_info = Some(
            pick(
                lang,
                "已新增 provider（待保存）。",
                "Provider added (save pending).",
            )
            .to_string(),
        );
    }

    *selected_provider_name = Some(name.to_string());
    *provider_editor_name = Some(name.to_string());
    provider_editor_alias.clear();
    *provider_editor_enabled = true;
    provider_editor_auth_token_env.clear();
    provider_editor_api_key_env.clear();
    provider_editor_endpoints.clear();
    new_provider_name.clear();
}
