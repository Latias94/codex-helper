use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_session_profile_binding_editor(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
    sid: &str,
    profiles: &[ControlProfileOption],
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
    session_preview_station_specs: Option<&BTreeMap<String, PersistedStationSpec>>,
    session_preview_provider_catalog: Option<&BTreeMap<String, PersistedStationProviderRef>>,
    session_preview_runtime_station_catalog: Option<&BTreeMap<String, StationOption>>,
    action_apply_session_profile: &mut Option<(String, String)>,
    action_clear_session_profile_binding: &mut Option<String>,
) {
    if profiles.is_empty() {
        ui.label(pick(
            ctx.lang,
            "当前未加载 control profile；可在代理配置文件里的 [codex.profiles.*] 中定义。",
            "No control profiles loaded; define them in the proxy config file under [codex.profiles.*].",
        ));
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(pick(ctx.lang, "快捷应用", "Quick apply"));
        for profile in profiles {
            let mut label = format_profile_display(profile.name.as_str(), Some(profile));
            if row.binding_profile_name.as_deref() == Some(profile.name.as_str()) {
                label.push_str(match ctx.lang {
                    Language::Zh => " [当前绑定]",
                    Language::En => " [bound]",
                });
            }
            let response = ui
                .button(label)
                .on_hover_text(format_profile_summary(profile));
            if response.clicked() {
                ctx.view.sessions.editor.profile_selection = Some(profile.name.clone());
                *action_apply_session_profile = Some((sid.to_string(), profile.name.clone()));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "Profile binding", "Profile binding"));

        let mut selected_profile = ctx.view.sessions.editor.profile_selection.clone();
        egui::ComboBox::from_id_salt(("session_profile_apply", sid))
            .selected_text(match selected_profile.as_deref() {
                Some(name) => {
                    format_profile_display(name, profiles.iter().find(|profile| profile.name == name))
                }
                None => pick(ctx.lang, "<选择>", "<select>").to_string(),
            })
            .show_ui(ui, |ui| {
                for profile in profiles {
                    ui.selectable_value(
                        &mut selected_profile,
                        Some(profile.name.clone()),
                        format_profile_display(profile.name.as_str(), Some(profile)),
                    );
                }
            });
        if selected_profile != ctx.view.sessions.editor.profile_selection {
            ctx.view.sessions.editor.profile_selection = selected_profile;
        }

        if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
            if let Some(profile_name) = ctx.view.sessions.editor.profile_selection.clone() {
                *action_apply_session_profile = Some((sid.to_string(), profile_name));
            } else {
                *ctx.last_error = Some(
                    pick(ctx.lang, "请先选择一个 profile。", "Select a profile first.")
                        .to_string(),
                );
            }
        }

        let clear_binding = ui
            .add_enabled(
                row.binding_profile_name.is_some(),
                egui::Button::new(pick(ctx.lang, "清除 binding", "Clear binding")),
            )
            .on_hover_text(pick(
                ctx.lang,
                "只移除当前会话已存储的 profile binding；保留 model / station / effort / service_tier 覆盖。",
                "Only removes the stored session profile binding; keep model / station / effort / service_tier overrides.",
            ));
        if clear_binding.clicked() {
            *action_clear_session_profile_binding = Some(sid.to_string());
        }
    });

    if let Some(profile_name) = ctx.view.sessions.editor.profile_selection.as_deref()
        && let Some(profile) = profiles.iter().find(|profile| profile.name == profile_name)
    {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "Profile 详情", "Profile details"),
            format_profile_summary(profile)
        ));
        let preview_profile = match resolve_service_profile_from_options(profile_name, profiles) {
            Ok(profile) => profile,
            Err(_) => service_profile_from_option(profile),
        };
        let preview = build_profile_route_preview(
            &preview_profile,
            configured_active_station,
            effective_active_station,
            session_preview_station_specs,
            session_preview_provider_catalog,
            session_preview_runtime_station_catalog,
        );
        render_session_profile_apply_preview(
            ui,
            ctx.lang,
            row,
            profile_name,
            &preview_profile,
            &preview,
        );
    }
}
