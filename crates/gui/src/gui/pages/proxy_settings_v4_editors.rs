use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn render_v4_provider_editor(
    ui: &mut egui::Ui,
    lang: Language,
    cfg: &mut crate::config::ProxyConfigV4,
    editor: &mut ProxySettingsProviderEditorState,
) -> Option<Result<String, String>> {
    ui.heading(pick(lang, "Provider 编辑", "Provider editor"));
    ui.small(pick(
        lang,
        "这是常用单 endpoint provider 的快速表单。新增 provider 会自动加入入口 route。",
        "This is a quick editor for common single-endpoint providers. New providers are appended to the entry route.",
    ));
    ui.add_space(6.0);

    let previous_service = editor.service;
    ui.horizontal(|ui| {
        ui.label(pick(lang, "服务", "Service"));
        egui::ComboBox::from_id_salt("proxy_settings_provider_service")
            .selected_text(provider_editor_service_label(lang, editor.service))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut editor.service,
                    ProxySettingsProviderEditorService::Codex,
                    "codex",
                );
                ui.selectable_value(
                    &mut editor.service,
                    ProxySettingsProviderEditorService::Claude,
                    "claude",
                );
            });
    });
    if editor.service != previous_service {
        reset_provider_editor_draft(editor);
    }

    let provider_names = {
        let service = select_provider_editor_service(cfg, editor.service);
        ordered_provider_names_for_editor(service)
    };
    if let Some(selected) = editor.selected_provider.as_deref()
        && !provider_names.iter().any(|name| name == selected)
    {
        reset_provider_editor_draft(editor);
    }

    let mut selection = editor.selected_provider.clone();
    ui.horizontal(|ui| {
        ui.label(pick(lang, "Provider", "Provider"));
        egui::ComboBox::from_id_salt("proxy_settings_provider_selector")
            .selected_text(
                selection
                    .as_deref()
                    .unwrap_or_else(|| pick(lang, "<新建>", "<new>")),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selection, None, pick(lang, "<新建>", "<new>"));
                for name in &provider_names {
                    ui.selectable_value(&mut selection, Some(name.clone()), name);
                }
            });
        if ui.button(pick(lang, "新建", "New")).clicked() {
            selection = None;
        }
    });
    if selection != editor.selected_provider {
        load_provider_editor_draft(cfg, editor, selection);
    }

    let selected_is_advanced = editor
        .selected_provider
        .as_deref()
        .and_then(|name| {
            select_provider_editor_service(cfg, editor.service)
                .providers
                .get(name)
        })
        .is_some_and(provider_is_advanced_for_form);

    if selected_is_advanced {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            pick(
                lang,
                "此 provider 包含额外 endpoints 或内联密钥；表单只读，避免误删高级配置。请用 Raw 或 CLI 编辑。",
                "This provider has extra endpoints or inline secrets; the form is read-only to avoid losing advanced config. Use Raw or CLI.",
            ),
        );
    }

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(pick(lang, "名称", "Name"));
            ui.add_enabled(
                editor.selected_provider.is_none(),
                egui::TextEdit::singleline(&mut editor.draft_name)
                    .desired_width(180.0)
                    .hint_text("input"),
            );
            ui.checkbox(&mut editor.enabled, pick(lang, "启用", "Enabled"));
        });
        ui.horizontal(|ui| {
            ui.label("alias");
            ui.add(
                egui::TextEdit::singleline(&mut editor.alias)
                    .desired_width(180.0)
                    .hint_text("Input Relay"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("base_url");
            ui.add(
                egui::TextEdit::singleline(&mut editor.base_url)
                    .desired_width(360.0)
                    .hint_text("https://relay.example.com/v1"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("auth_token_env");
            ui.add(
                egui::TextEdit::singleline(&mut editor.auth_token_env)
                    .desired_width(180.0)
                    .hint_text("INPUT_API_KEY"),
            );
            ui.label("api_key_env");
            ui.add(
                egui::TextEdit::singleline(&mut editor.api_key_env)
                    .desired_width(180.0)
                    .hint_text("INPUT_API_KEY"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("tags");
            ui.add(
                egui::TextEdit::singleline(&mut editor.tags)
                    .desired_width(420.0)
                    .hint_text("billing=monthly, vendor=input"),
            );
        });
        ui.small(pick(
            lang,
            "tags 使用 key=value，逗号或换行分隔；包月 provider 建议加 billing=monthly。",
            "Tags use key=value separated by commas or newlines; monthly relays should use billing=monthly.",
        ));
    });

    let mut action = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !selected_is_advanced,
                egui::Button::new(pick(lang, "保存 provider", "Save provider")),
            )
            .clicked()
        {
            action = Some(save_provider_from_editor(cfg, editor, lang));
        }

        if let Some(selected) = editor.selected_provider.clone()
            && ui
                .button(pick(lang, "删除 provider", "Remove provider"))
                .clicked()
        {
            action = Some(remove_provider_from_editor(
                cfg,
                editor,
                selected.as_str(),
                lang,
            ));
        }
    });

    let service = select_provider_editor_service(cfg, editor.service);
    let order_preview = service
        .routing
        .as_ref()
        .map(|_| {
            let routing = crate::config::effective_v4_routing(service);
            let order = routing
                .entry_node()
                .map(|node| node.children.as_slice())
                .unwrap_or(&[]);
            if order.is_empty() {
                "<provider key order>".to_string()
            } else {
                order.join(" -> ")
            }
        })
        .unwrap_or_else(|| "<implicit ordered-failover>".to_string());
    ui.small(format!(
        "{}: {}",
        pick(lang, "当前 fallback 顺序", "Current fallback order"),
        order_preview
    ));

    action
}

pub(super) fn render_v4_routing_editor(
    ui: &mut egui::Ui,
    lang: Language,
    cfg: &mut crate::config::ProxyConfigV4,
    service_kind: ProxySettingsProviderEditorService,
    editor: &mut ProxySettingsRoutingEditorState,
) -> Option<Result<String, String>> {
    let signature = {
        let service = select_provider_editor_service(cfg, service_kind);
        routing_editor_source_signature(service)
    };
    if editor.source_signature.as_deref() != Some(signature.as_str()) {
        let service = select_provider_editor_service(cfg, service_kind);
        load_routing_editor_from_service(editor, service, signature);
    }

    ui.heading(pick(lang, "Routing 编辑", "Routing editor"));
    ui.small(format!(
        "{}: {}",
        pick(lang, "当前服务", "Service"),
        provider_editor_service_label(lang, service_kind)
    ));
    ui.add_space(6.0);

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label("policy");
            egui::ComboBox::from_id_salt("proxy_settings_routing_policy")
                .selected_text(routing_policy_label(editor.policy))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut editor.policy,
                        crate::config::RoutingPolicyV4::OrderedFailover,
                        "ordered-failover",
                    );
                    ui.selectable_value(
                        &mut editor.policy,
                        crate::config::RoutingPolicyV4::ManualSticky,
                        "manual-sticky",
                    );
                    ui.selectable_value(
                        &mut editor.policy,
                        crate::config::RoutingPolicyV4::TagPreferred,
                        "tag-preferred",
                    );
                });

            ui.label("on_exhausted");
            egui::ComboBox::from_id_salt("proxy_settings_routing_on_exhausted")
                .selected_text(routing_exhausted_label(editor.on_exhausted))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut editor.on_exhausted,
                        crate::config::RoutingExhaustedActionV4::Continue,
                        "continue",
                    );
                    ui.selectable_value(
                        &mut editor.on_exhausted,
                        crate::config::RoutingExhaustedActionV4::Stop,
                        "stop",
                    );
                });
        });

        let provider_names = {
            let service = select_provider_editor_service(cfg, service_kind);
            ordered_provider_names_for_editor(service)
        };
        ui.horizontal(|ui| {
            ui.label("target");
            egui::ComboBox::from_id_salt("proxy_settings_routing_target")
                .selected_text(if editor.target.trim().is_empty() {
                    "<none>"
                } else {
                    editor.target.trim()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut editor.target, String::new(), "<none>");
                    for name in &provider_names {
                        ui.selectable_value(&mut editor.target, name.clone(), name);
                    }
                });
            ui.small(pick(
                lang,
                "manual-sticky 使用 target；其他 policy 保存时会清空 target。",
                "manual-sticky uses target; other policies clear target on save.",
            ));
        });

        ui.horizontal(|ui| {
            ui.label("order");
            ui.add(
                egui::TextEdit::singleline(&mut editor.order)
                    .desired_width(460.0)
                    .hint_text("monthly_a, monthly_b, paygo"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("prefer_tags");
            ui.add_enabled(
                matches!(editor.policy, crate::config::RoutingPolicyV4::TagPreferred),
                egui::TextEdit::singleline(&mut editor.prefer_tags)
                    .desired_width(360.0)
                    .hint_text("billing=monthly"),
            );
        });
        ui.small(pick(
            lang,
            "order 使用逗号或换行分隔；未列出的 provider 会保留在尾部 fallback。prefer_tags 的多组条件用分号分隔。",
            "Order is comma- or newline-separated; unlisted providers are kept as tail fallbacks. Separate multiple prefer_tags groups with semicolons.",
        ));
    });

    let draft = {
        let service = select_provider_editor_service(cfg, service_kind);
        build_routing_from_editor(editor, service)
    };
    render_routing_editor_preview(ui, lang, cfg, service_kind, draft.as_ref());

    let mut action = None;
    ui.horizontal(|ui| {
        if ui
            .button(pick(lang, "保存 routing", "Save routing"))
            .clicked()
        {
            action = Some(save_routing_from_editor(cfg, editor, service_kind, lang));
        }
        if ui.button(pick(lang, "重置表单", "Reset form")).clicked() {
            let signature = {
                let service = select_provider_editor_service(cfg, service_kind);
                routing_editor_source_signature(service)
            };
            let service = select_provider_editor_service(cfg, service_kind);
            load_routing_editor_from_service(editor, service, signature);
        }
    });

    action
}

pub(super) fn routing_policy_label(policy: crate::config::RoutingPolicyV4) -> &'static str {
    match policy {
        crate::config::RoutingPolicyV4::ManualSticky => "manual-sticky",
        crate::config::RoutingPolicyV4::OrderedFailover => "ordered-failover",
        crate::config::RoutingPolicyV4::TagPreferred => "tag-preferred",
    }
}

fn routing_exhausted_label(action: crate::config::RoutingExhaustedActionV4) -> &'static str {
    match action {
        crate::config::RoutingExhaustedActionV4::Continue => "continue",
        crate::config::RoutingExhaustedActionV4::Stop => "stop",
    }
}

fn routing_editor_source_signature(service: &crate::config::ServiceViewV4) -> String {
    let routing = service
        .routing
        .as_ref()
        .map(|routing| {
            let routes = routing
                .routes
                .iter()
                .map(|(name, node)| {
                    format!(
                        "{name}:{:?}:{}:{}:{}",
                        node.strategy,
                        node.children.join(","),
                        node.target.as_deref().unwrap_or_default(),
                        format_routing_prefer_tag_sets(&node.prefer_tags)
                    )
                })
                .collect::<Vec<_>>()
                .join("|");
            format!(
                "{}|{}|{}",
                routing.entry,
                routes,
                routing
                    .entry_node()
                    .map(|node| routing_exhausted_label(node.on_exhausted))
                    .unwrap_or("continue")
            )
        })
        .unwrap_or_else(|| "<implicit>".to_string());
    let providers = service
        .providers
        .iter()
        .map(|(name, provider)| {
            format!(
                "{}:{}:{}",
                name,
                provider.enabled,
                format_provider_editor_tags(&provider.tags)
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!("{routing}::{providers}")
}

fn load_routing_editor_from_service(
    editor: &mut ProxySettingsRoutingEditorState,
    service: &crate::config::ServiceViewV4,
    signature: String,
) {
    let routing = service
        .routing
        .as_ref()
        .map(|_| crate::config::effective_v4_routing(service));
    let entry_node = routing.as_ref().and_then(|routing| routing.entry_node());
    editor.policy = entry_node
        .map(|node| node.strategy)
        .unwrap_or(crate::config::RoutingPolicyV4::OrderedFailover);
    editor.target = entry_node
        .and_then(|node| node.target.clone())
        .unwrap_or_default();
    editor.order = entry_node
        .map(|node| {
            if node.children.is_empty() {
                ordered_provider_names_for_editor(service).join(", ")
            } else {
                node.children.join(", ")
            }
        })
        .unwrap_or_else(|| ordered_provider_names_for_editor(service).join(", "));
    editor.prefer_tags = entry_node
        .map(|node| format_routing_prefer_tag_sets(&node.prefer_tags))
        .unwrap_or_default();
    editor.original_routing = routing.clone();
    editor.on_exhausted = entry_node
        .map(|node| node.on_exhausted)
        .unwrap_or(crate::config::RoutingExhaustedActionV4::Continue);
    editor.source_signature = Some(signature);
}

fn save_routing_from_editor(
    cfg: &mut crate::config::ProxyConfigV4,
    editor: &mut ProxySettingsRoutingEditorState,
    service_kind: ProxySettingsProviderEditorService,
    lang: Language,
) -> Result<String, String> {
    let service = select_provider_editor_service_mut(cfg, service_kind);
    let routing = build_routing_from_editor(editor, service)?;
    service.routing = Some(routing);
    editor.source_signature = None;
    Ok(format!(
        "{} {}",
        pick(lang, "已保存 routing", "Saved routing"),
        provider_editor_service_label(lang, service_kind)
    ))
}

fn build_routing_from_editor(
    editor: &ProxySettingsRoutingEditorState,
    service: &crate::config::ServiceViewV4,
) -> Result<crate::config::RoutingConfigV4, String> {
    let order = normalize_routing_editor_order(&editor.order, service)?;
    let on_exhausted = editor.on_exhausted;
    match editor.policy {
        crate::config::RoutingPolicyV4::ManualSticky => {
            let target = editor.target.trim();
            if target.is_empty() {
                return Err("manual-sticky routing requires a target provider".to_string());
            }
            if !service.providers.contains_key(target) {
                return Err(format!("target provider '{target}' does not exist"));
            }
            Ok(crate::config::RoutingConfigV4::manual_sticky(
                target.to_string(),
                order,
            ))
        }
        crate::config::RoutingPolicyV4::OrderedFailover => {
            Ok(crate::config::RoutingConfigV4::ordered_failover(order))
        }
        crate::config::RoutingPolicyV4::TagPreferred => {
            let prefer_tags = parse_routing_prefer_tag_sets(&editor.prefer_tags)?;
            if prefer_tags.is_empty() {
                return Err("tag-preferred routing requires prefer_tags".to_string());
            }
            if matches!(on_exhausted, crate::config::RoutingExhaustedActionV4::Stop)
                && !order.iter().any(|name| {
                    service.providers.get(name).is_some_and(|provider| {
                        provider_matches_any_tag_set(provider, &prefer_tags)
                    })
                })
            {
                return Err(
                    "tag-preferred routing with on_exhausted=stop matches no providers".to_string(),
                );
            }
            Ok(crate::config::RoutingConfigV4::tag_preferred(
                order,
                prefer_tags,
                on_exhausted,
            ))
        }
    }
}

fn normalize_routing_editor_order(
    raw: &str,
    service: &crate::config::ServiceViewV4,
) -> Result<Vec<String>, String> {
    let mut order = parse_routing_provider_list(raw);
    if order.is_empty() {
        order = ordered_provider_names_for_editor(service);
    }
    let mut seen = BTreeSet::new();
    for name in &order {
        if !service.providers.contains_key(name) {
            return Err(format!("provider '{name}' in routing entry does not exist"));
        }
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate provider '{name}' in routing entry"));
        }
    }
    for name in ordered_provider_names_for_editor(service) {
        if seen.insert(name.clone()) {
            order.push(name);
        }
    }
    Ok(order)
}

fn parse_routing_provider_list(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_routing_prefer_tag_sets(raw: &str) -> Result<Vec<BTreeMap<String, String>>, String> {
    let mut groups = Vec::new();
    for group in raw.split([';', '\n']) {
        let group = group.trim();
        if group.is_empty() {
            continue;
        }
        let tag_set = parse_provider_editor_tags(group)?;
        if tag_set.is_empty() {
            return Err("prefer_tags entries must contain at least one key/value pair".to_string());
        }
        groups.push(tag_set);
    }
    Ok(groups)
}

fn format_routing_prefer_tag_sets(tag_sets: &[BTreeMap<String, String>]) -> String {
    tag_sets
        .iter()
        .map(format_provider_editor_tags)
        .filter(|group| !group.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutingPreviewRow {
    provider: String,
    role: &'static str,
    enabled: bool,
    tags: String,
}

fn render_routing_editor_preview(
    ui: &mut egui::Ui,
    lang: Language,
    cfg: &crate::config::ProxyConfigV4,
    service_kind: ProxySettingsProviderEditorService,
    draft: Result<&crate::config::RoutingConfigV4, &String>,
) {
    ui.group(|ui| {
        ui.label(pick(lang, "Routing 预览", "Routing preview"));
        let service = select_provider_editor_service(cfg, service_kind);
        match draft {
            Ok(routing) => {
                let rows = routing_preview_rows(service, routing);
                if rows.is_empty() {
                    ui.small(pick(
                        lang,
                        "没有可用 provider。",
                        "No providers are available.",
                    ));
                } else {
                    for row in rows.iter().take(12) {
                        let state = if row.enabled { "on" } else { "off" };
                        ui.small(format!(
                            "{}  {}  [{}]  tags={}",
                            row.role, row.provider, state, row.tags
                        ));
                    }
                    if rows.len() > 12 {
                        ui.small(format!("... +{} more", rows.len() - 12));
                    }
                }
            }
            Err(err) => {
                ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
            }
        }
    });
}

fn routing_preview_rows(
    service: &crate::config::ServiceViewV4,
    routing: &crate::config::RoutingConfigV4,
) -> Vec<RoutingPreviewRow> {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let effective = {
        let mut service = service.clone();
        service.routing = Some(routing.clone());
        crate::config::effective_v4_routing(&service)
    };
    let node = effective.entry_node();
    match node
        .map(|node| node.strategy)
        .unwrap_or(crate::config::RoutingPolicyV4::OrderedFailover)
    {
        crate::config::RoutingPolicyV4::ManualSticky => {
            if let Some(target) = node.and_then(|node| node.target.as_deref()) {
                push_routing_preview_row(&mut rows, &mut seen, service, target, "target");
            }
        }
        crate::config::RoutingPolicyV4::OrderedFailover => {
            let order = crate::config::resolved_v4_provider_order("gui-routing-preview", &{
                let mut service = service.clone();
                service.routing = Some(effective.clone());
                service
            })
            .unwrap_or_else(|_| ordered_provider_names_for_editor(service));
            for name in &order {
                push_routing_preview_row(&mut rows, &mut seen, service, name, "fallback");
            }
        }
        crate::config::RoutingPolicyV4::TagPreferred => {
            let children = node.map(|node| node.children.as_slice()).unwrap_or(&[]);
            let prefer_tags = node.map(|node| node.prefer_tags.as_slice()).unwrap_or(&[]);
            for name in children {
                if let Some(provider) = service.providers.get(name)
                    && provider_matches_any_tag_set(provider, prefer_tags)
                {
                    push_routing_preview_row(&mut rows, &mut seen, service, name, "preferred");
                }
            }
            if matches!(
                node.map(|node| node.on_exhausted)
                    .unwrap_or(crate::config::RoutingExhaustedActionV4::Continue),
                crate::config::RoutingExhaustedActionV4::Continue
            ) {
                for name in children {
                    push_routing_preview_row(&mut rows, &mut seen, service, name, "fallback");
                }
            }
        }
    }
    rows
}

fn push_routing_preview_row(
    rows: &mut Vec<RoutingPreviewRow>,
    seen: &mut BTreeSet<String>,
    service: &crate::config::ServiceViewV4,
    provider_name: &str,
    role: &'static str,
) {
    if !seen.insert(provider_name.to_string()) {
        return;
    }
    let (enabled, tags) = service
        .providers
        .get(provider_name)
        .map(|provider| {
            let tags = if provider.tags.is_empty() {
                "-".to_string()
            } else {
                format_provider_editor_tags(&provider.tags)
            };
            (provider.enabled, tags)
        })
        .unwrap_or((false, "<missing>".to_string()));
    rows.push(RoutingPreviewRow {
        provider: provider_name.to_string(),
        role,
        enabled,
        tags,
    });
}

fn provider_matches_any_tag_set(
    provider: &crate::config::ProviderConfigV4,
    tag_sets: &[BTreeMap<String, String>],
) -> bool {
    tag_sets.iter().any(|tag_set| {
        tag_set
            .iter()
            .all(|(key, value)| provider.tags.get(key) == Some(value))
    })
}

fn provider_editor_service_label(
    lang: Language,
    service: ProxySettingsProviderEditorService,
) -> &'static str {
    match service {
        ProxySettingsProviderEditorService::Codex => "codex",
        ProxySettingsProviderEditorService::Claude => pick(lang, "claude", "claude"),
    }
}

fn select_provider_editor_service(
    cfg: &crate::config::ProxyConfigV4,
    service: ProxySettingsProviderEditorService,
) -> &crate::config::ServiceViewV4 {
    match service {
        ProxySettingsProviderEditorService::Codex => &cfg.codex,
        ProxySettingsProviderEditorService::Claude => &cfg.claude,
    }
}

fn select_provider_editor_service_mut(
    cfg: &mut crate::config::ProxyConfigV4,
    service: ProxySettingsProviderEditorService,
) -> &mut crate::config::ServiceViewV4 {
    match service {
        ProxySettingsProviderEditorService::Codex => &mut cfg.codex,
        ProxySettingsProviderEditorService::Claude => &mut cfg.claude,
    }
}

fn ordered_provider_names_for_editor(view: &crate::config::ServiceViewV4) -> Vec<String> {
    let mut names =
        crate::config::resolved_v4_provider_order("gui-provider-editor", view).unwrap_or_default();
    for name in view.providers.keys() {
        push_provider_name_once(&mut names, view, name);
    }
    names
}

fn push_provider_name_once(
    names: &mut Vec<String>,
    view: &crate::config::ServiceViewV4,
    name: &str,
) {
    if view.providers.contains_key(name) && !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}

fn reset_provider_editor_draft(editor: &mut ProxySettingsProviderEditorState) {
    editor.selected_provider = None;
    editor.draft_name.clear();
    editor.alias.clear();
    editor.base_url.clear();
    editor.auth_token_env.clear();
    editor.api_key_env.clear();
    editor.tags.clear();
    editor.enabled = true;
}

fn load_provider_editor_draft(
    cfg: &crate::config::ProxyConfigV4,
    editor: &mut ProxySettingsProviderEditorState,
    selection: Option<String>,
) {
    editor.selected_provider = selection.clone();
    let Some(name) = selection else {
        reset_provider_editor_draft(editor);
        return;
    };

    let Some(provider) = select_provider_editor_service(cfg, editor.service)
        .providers
        .get(name.as_str())
    else {
        reset_provider_editor_draft(editor);
        return;
    };

    editor.draft_name = name;
    editor.alias = provider.alias.clone().unwrap_or_default();
    editor.base_url = provider.base_url.clone().unwrap_or_default();
    editor.auth_token_env = provider
        .inline_auth
        .auth_token_env
        .clone()
        .or_else(|| provider.auth.auth_token_env.clone())
        .unwrap_or_default();
    editor.api_key_env = provider
        .inline_auth
        .api_key_env
        .clone()
        .or_else(|| provider.auth.api_key_env.clone())
        .unwrap_or_default();
    editor.tags = format_provider_editor_tags(&provider.tags);
    editor.enabled = provider.enabled;
}

fn provider_is_advanced_for_form(provider: &crate::config::ProviderConfigV4) -> bool {
    !provider.endpoints.is_empty()
        || provider.inline_auth.auth_token.is_some()
        || provider.inline_auth.api_key.is_some()
        || provider.auth.auth_token.is_some()
        || provider.auth.api_key.is_some()
}

fn save_provider_from_editor(
    cfg: &mut crate::config::ProxyConfigV4,
    editor: &mut ProxySettingsProviderEditorState,
    lang: Language,
) -> Result<String, String> {
    let name = normalize_provider_editor_name(&editor.draft_name)?;
    let base_url = normalize_required_provider_editor_field(&editor.base_url, "base_url")?;
    let tags = parse_provider_editor_tags(&editor.tags)?;
    let selected = editor.selected_provider.clone();
    let service = select_provider_editor_service_mut(cfg, editor.service);

    if let Some(selected) = selected.as_deref() {
        if selected != name {
            return Err("renaming providers is not supported in the form editor; create a new provider instead".to_string());
        }
        let Some(existing) = service.providers.get(selected) else {
            return Err(format!("provider '{selected}' no longer exists"));
        };
        if provider_is_advanced_for_form(existing) {
            return Err(format!(
                "provider '{selected}' has advanced fields; edit it in Raw or CLI"
            ));
        }
    } else if service.providers.contains_key(name.as_str()) {
        return Err(format!(
            "provider '{}' already exists; select it to edit",
            name
        ));
    }

    let mut provider = selected
        .as_deref()
        .and_then(|selected| service.providers.get(selected).cloned())
        .unwrap_or_default();
    provider.alias = normalize_optional_provider_editor_field(&editor.alias);
    provider.enabled = editor.enabled;
    provider.base_url = Some(base_url);
    provider.auth = crate::config::UpstreamAuth::default();
    provider.inline_auth = crate::config::UpstreamAuth {
        auth_token: None,
        auth_token_env: normalize_optional_provider_editor_field(&editor.auth_token_env),
        api_key: None,
        api_key_env: normalize_optional_provider_editor_field(&editor.api_key_env),
    };
    provider.tags = tags;
    service.providers.insert(name.clone(), provider);
    ensure_provider_editor_routing_order_contains(service, name.as_str());
    if !editor.enabled {
        clear_provider_editor_manual_target(service, name.as_str());
    }

    editor.selected_provider = Some(name.clone());
    Ok(format!(
        "{} {} '{}'",
        pick(lang, "已保存 provider", "Saved provider"),
        provider_editor_service_label(lang, editor.service),
        name
    ))
}

fn remove_provider_from_editor(
    cfg: &mut crate::config::ProxyConfigV4,
    editor: &mut ProxySettingsProviderEditorState,
    provider_name: &str,
    lang: Language,
) -> Result<String, String> {
    let service = select_provider_editor_service_mut(cfg, editor.service);
    if service.providers.remove(provider_name).is_none() {
        return Err(format!("provider '{provider_name}' no longer exists"));
    }
    if let Some(routing) = service.routing.as_mut() {
        remove_provider_from_route_nodes(routing, provider_name);
    }
    reset_provider_editor_draft(editor);
    Ok(format!(
        "{} {} '{}'",
        pick(lang, "已删除 provider", "Removed provider"),
        provider_editor_service_label(lang, editor.service),
        provider_name
    ))
}

fn ensure_provider_editor_routing_order_contains(
    service: &mut crate::config::ServiceViewV4,
    provider_name: &str,
) {
    let routing = service
        .routing
        .get_or_insert_with(crate::config::RoutingConfigV4::default);
    let entry = routing.entry.clone();
    let node = routing.routes.entry(entry).or_default();
    if !node.children.iter().any(|name| name == provider_name) {
        node.children.push(provider_name.to_string());
    }
    routing.sync_compat_from_graph();
}

fn clear_provider_editor_manual_target(
    service: &mut crate::config::ServiceViewV4,
    provider_name: &str,
) {
    let Some(routing) = service.routing.as_mut() else {
        return;
    };
    if routing.entry_node().and_then(|node| node.target.as_deref()) == Some(provider_name) {
        let entry = routing.entry.clone();
        let node = routing.routes.entry(entry).or_default();
        node.strategy = crate::config::RoutingPolicyV4::OrderedFailover;
        node.target = None;
        node.prefer_tags.clear();
        node.on_exhausted = crate::config::RoutingExhaustedActionV4::Continue;
        routing.sync_compat_from_graph();
    }
}

fn remove_provider_from_route_nodes(
    routing: &mut crate::config::RoutingConfigV4,
    provider_name: &str,
) {
    for node in routing.routes.values_mut() {
        node.children.retain(|name| name != provider_name);
        if node.target.as_deref() == Some(provider_name) {
            node.target = None;
            if matches!(node.strategy, crate::config::RoutingPolicyV4::ManualSticky) {
                node.strategy = crate::config::RoutingPolicyV4::OrderedFailover;
            }
        }
    }
    routing.sync_compat_from_graph();
}

fn normalize_provider_editor_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("provider name is required".to_string());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(
            "provider name may only contain ASCII letters, numbers, '.', '_' and '-'".to_string(),
        );
    }
    Ok(name.to_string())
}

fn normalize_required_provider_editor_field(raw: &str, field: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        Err(format!("{field} is required"))
    } else {
        Ok(value.to_string())
    }
}

fn normalize_optional_provider_editor_field(raw: &str) -> Option<String> {
    let value = raw.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_provider_editor_tags(raw: &str) -> Result<BTreeMap<String, String>, String> {
    let mut tags = BTreeMap::new();
    for part in raw.split([',', '\n']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once('=') else {
            return Err(format!("tag '{part}' must use KEY=VALUE form"));
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            return Err(format!("tag '{part}' has an empty key"));
        }
        if value.is_empty() {
            return Err(format!("tag '{part}' has an empty value"));
        }
        if tags.insert(key.to_string(), value.to_string()).is_some() {
            return Err(format!("duplicate tag key '{key}'"));
        }
    }
    Ok(tags)
}

fn format_provider_editor_tags(tags: &BTreeMap<String, String>) -> String {
    tags.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_editor_tags_parse_comma_and_newline_separated_pairs() {
        let tags = parse_provider_editor_tags("billing=monthly, vendor=input\nregion=hk")
            .expect("tags should parse");

        assert_eq!(tags.get("billing").map(String::as_str), Some("monthly"));
        assert_eq!(tags.get("vendor").map(String::as_str), Some("input"));
        assert_eq!(tags.get("region").map(String::as_str), Some("hk"));
    }

    #[test]
    fn provider_editor_tags_reject_duplicate_keys() {
        let err = parse_provider_editor_tags("billing=monthly,billing=paygo")
            .expect_err("duplicate keys should fail");

        assert!(err.contains("duplicate tag key"));
    }

    #[test]
    fn provider_editor_save_adds_inline_provider_and_routing_order() {
        let mut cfg = crate::config::ProxyConfigV4::default();
        let mut editor = ProxySettingsProviderEditorState {
            draft_name: "input".to_string(),
            base_url: "https://ai.input.im/v1".to_string(),
            auth_token_env: "INPUT_API_KEY".to_string(),
            tags: "billing=monthly, vendor=input".to_string(),
            ..ProxySettingsProviderEditorState::default()
        };

        save_provider_from_editor(&mut cfg, &mut editor, Language::En)
            .expect("provider should save");

        let provider = cfg.codex.providers.get("input").expect("provider exists");
        assert_eq!(provider.base_url.as_deref(), Some("https://ai.input.im/v1"));
        assert_eq!(
            provider.inline_auth.auth_token_env.as_deref(),
            Some("INPUT_API_KEY")
        );
        assert_eq!(
            provider.tags.get("billing").map(String::as_str),
            Some("monthly")
        );
        assert_eq!(
            cfg.codex
                .routing
                .as_ref()
                .and_then(|routing| routing.entry_node())
                .map(|node| node.children.as_slice()),
            Some(&["input".to_string()][..])
        );
    }

    #[test]
    fn provider_editor_disable_clears_manual_target() {
        let mut cfg = crate::config::ProxyConfigV4::default();
        cfg.codex.providers.insert(
            "input".to_string(),
            crate::config::ProviderConfigV4 {
                base_url: Some("https://ai.input.im/v1".to_string()),
                ..crate::config::ProviderConfigV4::default()
            },
        );
        cfg.codex.routing = Some(crate::config::RoutingConfigV4::manual_sticky(
            "input".to_string(),
            vec!["input".to_string()],
        ));
        let mut editor = ProxySettingsProviderEditorState {
            selected_provider: Some("input".to_string()),
            draft_name: "input".to_string(),
            base_url: "https://ai.input.im/v1".to_string(),
            enabled: false,
            ..ProxySettingsProviderEditorState::default()
        };

        save_provider_from_editor(&mut cfg, &mut editor, Language::En)
            .expect("provider should save");

        let routing = cfg.codex.routing.as_ref().expect("routing exists");
        let entry = routing.entry_node().expect("entry exists");
        assert_eq!(
            entry.strategy,
            crate::config::RoutingPolicyV4::OrderedFailover
        );
        assert_eq!(entry.target, None);
    }

    #[test]
    fn routing_editor_order_keeps_unlisted_providers_as_tail_fallbacks() {
        let mut service = crate::config::ServiceViewV4::default();
        service
            .providers
            .insert("a".to_string(), crate::config::ProviderConfigV4::default());
        service
            .providers
            .insert("b".to_string(), crate::config::ProviderConfigV4::default());
        service
            .providers
            .insert("c".to_string(), crate::config::ProviderConfigV4::default());
        service.routing = Some(crate::config::RoutingConfigV4::ordered_failover(vec![
            "c".to_string(),
            "a".to_string(),
            "b".to_string(),
        ]));
        let editor = ProxySettingsRoutingEditorState {
            order: "b".to_string(),
            ..ProxySettingsRoutingEditorState::default()
        };

        let routing = build_routing_from_editor(&editor, &service).expect("routing should build");

        assert_eq!(
            routing.entry_node().map(|node| node.children.as_slice()),
            Some(&["b".to_string(), "c".to_string(), "a".to_string()][..])
        );
    }

    #[test]
    fn routing_editor_tag_preferred_continue_previews_preferred_then_fallbacks() {
        let mut service = crate::config::ServiceViewV4::default();
        service.providers.insert(
            "monthly".to_string(),
            crate::config::ProviderConfigV4 {
                tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                ..crate::config::ProviderConfigV4::default()
            },
        );
        service.providers.insert(
            "paygo".to_string(),
            crate::config::ProviderConfigV4 {
                tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                ..crate::config::ProviderConfigV4::default()
            },
        );
        let editor = ProxySettingsRoutingEditorState {
            policy: crate::config::RoutingPolicyV4::TagPreferred,
            order: "paygo, monthly".to_string(),
            prefer_tags: "billing=monthly".to_string(),
            on_exhausted: crate::config::RoutingExhaustedActionV4::Continue,
            ..ProxySettingsRoutingEditorState::default()
        };

        let routing = build_routing_from_editor(&editor, &service).expect("routing should build");
        let rows = routing_preview_rows(&service, &routing);

        assert_eq!(
            rows.iter()
                .map(|row| (row.provider.as_str(), row.role))
                .collect::<Vec<_>>(),
            vec![("monthly", "preferred"), ("paygo", "fallback")]
        );
    }

    #[test]
    fn routing_editor_tag_preferred_stop_excludes_non_matching_fallbacks() {
        let mut service = crate::config::ServiceViewV4::default();
        service.providers.insert(
            "monthly".to_string(),
            crate::config::ProviderConfigV4 {
                tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                ..crate::config::ProviderConfigV4::default()
            },
        );
        service.providers.insert(
            "paygo".to_string(),
            crate::config::ProviderConfigV4 {
                tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                ..crate::config::ProviderConfigV4::default()
            },
        );
        let editor = ProxySettingsRoutingEditorState {
            policy: crate::config::RoutingPolicyV4::TagPreferred,
            order: "monthly, paygo".to_string(),
            prefer_tags: "billing=monthly".to_string(),
            on_exhausted: crate::config::RoutingExhaustedActionV4::Stop,
            ..ProxySettingsRoutingEditorState::default()
        };

        let routing = build_routing_from_editor(&editor, &service).expect("routing should build");
        let rows = routing_preview_rows(&service, &routing);

        assert_eq!(
            rows.iter()
                .map(|row| row.provider.as_str())
                .collect::<Vec<_>>(),
            vec!["monthly"]
        );
    }

    #[test]
    fn routing_editor_tag_preferred_stop_rejects_empty_match_set() {
        let mut service = crate::config::ServiceViewV4::default();
        service.providers.insert(
            "paygo".to_string(),
            crate::config::ProviderConfigV4 {
                tags: BTreeMap::from([("billing".to_string(), "paygo".to_string())]),
                ..crate::config::ProviderConfigV4::default()
            },
        );
        let editor = ProxySettingsRoutingEditorState {
            policy: crate::config::RoutingPolicyV4::TagPreferred,
            order: "paygo".to_string(),
            prefer_tags: "billing=monthly".to_string(),
            on_exhausted: crate::config::RoutingExhaustedActionV4::Stop,
            ..ProxySettingsRoutingEditorState::default()
        };

        let err = build_routing_from_editor(&editor, &service)
            .expect_err("stop should reject unmatched tag filters");

        assert!(err.contains("matches no providers"));
    }
}
