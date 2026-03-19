use super::*;

pub(crate) fn config_provider_endpoint_editor_from_spec(
    endpoint: &crate::config::PersistedProviderEndpointSpec,
) -> ProviderEndpointEditorState {
    ProviderEndpointEditorState {
        name: endpoint.name.clone(),
        base_url: endpoint.base_url.clone(),
        enabled: endpoint.enabled,
    }
}

pub(crate) fn build_provider_spec_from_config_editor(
    provider_name: &str,
    alias: &str,
    enabled: bool,
    auth_token_env: &str,
    api_key_env: &str,
    endpoints: &[ProviderEndpointEditorState],
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
            priority: index as u32,
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

pub(crate) fn merge_provider_spec_into_provider_config(
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
                        priority: endpoint.priority,
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

pub(crate) fn render_config_provider_endpoint_editor(
    ui: &mut egui::Ui,
    lang: Language,
    selected_service: &str,
    provider_name: &str,
    endpoints: &mut Vec<ProviderEndpointEditorState>,
) {
    let enabled_count = endpoints.iter().filter(|endpoint| endpoint.enabled).count();
    let standby_count = endpoints.len().saturating_sub(enabled_count);

    ui.group(|ui| {
        ui.columns(3, |cols| {
            render_endpoint_summary_card(
                &mut cols[0],
                pick(lang, "Active", "Active"),
                enabled_count.to_string(),
                pick(lang, "当前启用的 endpoint", "Currently enabled endpoints"),
            );
            render_endpoint_summary_card(
                &mut cols[1],
                pick(lang, "Standby", "Standby"),
                standby_count.to_string(),
                pick(lang, "已停用的备用槽位", "Disabled standby slots"),
            );
            render_endpoint_summary_card(
                &mut cols[2],
                pick(lang, "Pool", "Pool"),
                if enabled_count >= 2 {
                    pick(lang, "ready", "ready").to_string()
                } else {
                    pick(lang, "thin", "thin").to_string()
                },
                if enabled_count >= 2 {
                    pick(lang, "可形成同 provider failover 池", "Suitable for provider failover")
                } else {
                    pick(lang, "建议至少启用 2 个 endpoint", "Prefer at least 2 enabled endpoints")
                },
            );
        });

        ui.add_space(6.0);
        ui.small(pick(
            lang,
            "这里管理 provider 的来源优先级池。列表顺序会写入 priority，运行时默认按当前顺序优先选择，再由失败切换接管。",
            "This manages the provider source priority pool. The current list order is written into priority and becomes the default runtime preference before failover takes over.",
        ));
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .button(pick(lang, "新增 endpoint", "Add endpoint"))
            .clicked()
        {
            endpoints.push(ProviderEndpointEditorState {
                enabled: true,
                ..Default::default()
            });
        }

        if ui
            .add_enabled(
                !endpoints.is_empty(),
                egui::Button::new(pick(lang, "全部启用", "Enable all")),
            )
            .clicked()
        {
            for endpoint in endpoints.iter_mut() {
                endpoint.enabled = true;
            }
        }

        if ui
            .add_enabled(
                !endpoints.is_empty(),
                egui::Button::new(pick(lang, "全部停用", "Disable all")),
            )
            .clicked()
        {
            for endpoint in endpoints.iter_mut() {
                endpoint.enabled = false;
            }
        }
    });

    egui::ScrollArea::vertical()
        .id_salt(format!(
            "config_v2_provider_endpoints_edit_{selected_service}_{provider_name}"
        ))
        .max_height(260.0)
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
            let mut move_up_idx = None;
            let mut move_down_idx = None;
            let total_endpoints = endpoints.len();
            for (idx, endpoint) in endpoints.iter_mut().enumerate() {
                let role = endpoint_role(lang, endpoint.enabled, enabled_count);
                egui::Frame::group(ui.style())
                    .fill(if endpoint.enabled {
                        egui::Color32::from_rgb(250, 252, 247)
                    } else {
                        egui::Color32::from_rgb(250, 250, 250)
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if endpoint.enabled {
                            egui::Color32::from_rgb(120, 152, 108)
                        } else {
                            egui::Color32::from_rgb(190, 190, 190)
                        },
                    ))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            render_endpoint_badge(
                                ui,
                                format!("#{}", idx + 1),
                                egui::Color32::from_rgb(76, 114, 176),
                            );
                            render_endpoint_badge(ui, role.label, role.color);
                            render_endpoint_badge(
                                ui,
                                compact_endpoint_host(endpoint.base_url.as_str()),
                                egui::Color32::from_rgb(122, 90, 166),
                            );
                        });
                        ui.add_space(4.0);

                        ui.horizontal(|ui| {
                            ui.label("name");
                            ui.add_sized(
                                [180.0, 22.0],
                                egui::TextEdit::singleline(&mut endpoint.name),
                            );
                            ui.checkbox(&mut endpoint.enabled, pick(lang, "启用", "Enabled"));
                            if ui
                                .add_enabled(idx > 0, egui::Button::new(pick(lang, "上移", "Up")))
                                .clicked()
                            {
                                move_up_idx = Some(idx);
                            }
                            if ui
                                .add_enabled(
                                    idx + 1 < total_endpoints,
                                    egui::Button::new(pick(lang, "下移", "Down")),
                                )
                                .clicked()
                            {
                                move_down_idx = Some(idx);
                            }
                            if ui.button(pick(lang, "删除", "Delete")).clicked() {
                                delete_idx = Some(idx);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("base_url");
                            ui.add_sized(
                                [320.0, 22.0],
                                egui::TextEdit::singleline(&mut endpoint.base_url)
                                    .hint_text("https://example.com/v1"),
                            );
                        });
                        ui.small(role.hint);
                    });
                ui.add_space(6.0);
            }

            if let Some(idx) = delete_idx {
                endpoints.remove(idx);
            } else if let Some(idx) = move_up_idx {
                endpoints.swap(idx - 1, idx);
            } else if let Some(idx) = move_down_idx {
                endpoints.swap(idx, idx + 1);
            }
        });
}

struct EndpointRole<'a> {
    label: &'a str,
    hint: &'a str,
    color: egui::Color32,
}

fn endpoint_role(lang: Language, enabled: bool, enabled_count: usize) -> EndpointRole<'static> {
    if !enabled {
        EndpointRole {
            label: pick(lang, "standby", "standby"),
            hint: pick(
                lang,
                "当前停用，不会进入运行时来源池；适合作为手动备用槽位。",
                "Disabled endpoints stay out of the runtime pool and work well as manual standby slots.",
            ),
            color: egui::Color32::from_rgb(150, 150, 150),
        }
    } else if enabled_count >= 2 {
        EndpointRole {
            label: pick(lang, "priority-pool", "priority-pool"),
            hint: pick(
                lang,
                "当前已启用，会按列表顺序进入同 provider 的来源池；首项优先，其余项作为后续候选。",
                "Enabled endpoints join the provider source pool in list order; the first stays preferred and the rest remain fallback candidates.",
            ),
            color: egui::Color32::from_rgb(86, 122, 62),
        }
    } else {
        EndpointRole {
            label: pick(lang, "single-path", "single-path"),
            hint: pick(
                lang,
                "当前是唯一启用的 endpoint；可用但还不具备同 provider 内的 failover 冗余。",
                "This is the only enabled endpoint: usable, but without same-provider failover redundancy yet.",
            ),
            color: egui::Color32::from_rgb(176, 122, 76),
        }
    }
}

fn compact_endpoint_host(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return "host pending".to_string();
    }

    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

fn render_endpoint_summary_card(
    ui: &mut egui::Ui,
    title: &str,
    value: impl Into<String>,
    hint: &str,
) {
    ui.group(|ui| {
        ui.small(title);
        ui.heading(value.into());
        ui.small(hint);
    });
}

fn render_endpoint_badge(ui: &mut egui::Ui, text: impl Into<String>, color: egui::Color32) {
    ui.label(
        egui::RichText::new(text.into())
            .small()
            .color(color)
            .background_color(color.gamma_multiply(0.10)),
    );
}
