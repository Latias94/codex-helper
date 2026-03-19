use super::*;

pub(super) fn render_control_deck_focus_targets(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    render_ctx: &ProxySettingsRenderContext,
) {
    let active_station_name = render_ctx
        .effective_active_name
        .clone()
        .or_else(|| render_ctx.configured_active_name.clone());
    let default_profile_name = render_ctx.station_default_profile.clone();
    let focus_provider_name = focus_provider_name(render_ctx);
    let resolved_default_profile = default_profile_name
        .as_deref()
        .and_then(|name| resolve_profile_for_deck(render_ctx, name));

    ui.group(|ui| {
        ui.small(pick(
            ctx.lang,
            "聚焦当前控制目标",
            "Focus current control target",
        ));
        ui.horizontal_wrapped(|ui| {
            if let Some(station_name) = active_station_name.as_deref()
                && ui
                    .button(format!(
                        "{}: {}",
                        pick(ctx.lang, "聚焦站点", "Focus station"),
                        station_name
                    ))
                    .clicked()
            {
                ctx.view.proxy_settings.section = ProxySettingsSection::Stations;
                ctx.view.proxy_settings.selected_name = Some(station_name.to_string());
            }

            if let Some(profile_name) = default_profile_name.as_deref()
                && ui
                    .button(format!(
                        "{}: {}",
                        pick(ctx.lang, "聚焦默认 profile", "Focus default profile"),
                        profile_name
                    ))
                    .clicked()
            {
                ctx.view.proxy_settings.section = ProxySettingsSection::Profiles;
                ctx.view.proxy_settings.selected_profile_name = Some(profile_name.to_string());
            }

            if let Some(provider_name) = focus_provider_name.as_deref()
                && ui
                    .button(format!(
                        "{}: {}",
                        pick(ctx.lang, "聚焦 provider", "Focus provider"),
                        provider_name
                    ))
                    .clicked()
            {
                ctx.view.proxy_settings.section = ProxySettingsSection::Providers;
                ctx.view.proxy_settings.selected_provider_name = Some(provider_name.to_string());
            }
        });

        if let Some((profile_name, profile)) = resolved_default_profile {
            ui.add_space(6.0);
            ui.small(format!(
                "{}: {}",
                pick(
                    ctx.lang,
                    "当前默认 profile 摘要",
                    "Current default profile summary"
                ),
                profile_name
            ));
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "station={}",
                    session_profile_target_value(profile.station.as_deref(), ctx.lang)
                ));
                ui.label(format!(
                    "model={}",
                    session_profile_target_value(profile.model.as_deref(), ctx.lang)
                ));
                ui.label(format!(
                    "reasoning={}",
                    session_profile_target_value(profile.reasoning_effort.as_deref(), ctx.lang)
                ));
                ui.label(format!(
                    "service_tier={}",
                    format_service_tier_display(profile.service_tier.as_deref(), ctx.lang, "auto")
                ));
            });
        }
    });
}

fn resolve_profile_for_deck(
    render_ctx: &ProxySettingsRenderContext,
    profile_name: &str,
) -> Option<(String, crate::config::ServiceControlProfile)> {
    let profile = crate::config::resolve_service_profile_from_catalog(
        current_profile_catalog(render_ctx),
        profile_name,
    )
    .ok()
    .or_else(|| {
        current_profile_catalog(render_ctx)
            .get(profile_name)
            .cloned()
    })?;
    Some((profile_name.to_string(), profile))
}

fn current_profile_catalog(
    render_ctx: &ProxySettingsRenderContext,
) -> &BTreeMap<String, crate::config::ServiceControlProfile> {
    if render_ctx.profile_control_plane_enabled {
        &render_ctx.profile_control_plane_catalog
    } else {
        &render_ctx.profile_catalog
    }
}

fn focus_provider_name(render_ctx: &ProxySettingsRenderContext) -> Option<String> {
    let active_station_name = render_ctx
        .effective_active_name
        .as_deref()
        .or(render_ctx.configured_active_name.as_deref());

    if let Some(station_name) = active_station_name
        && let Some(station_specs) = render_ctx.preview_station_specs()
        && let Some(station) = station_specs.get(station_name)
        && let Some(member) = station.members.first()
    {
        return Some(member.provider.clone());
    }

    render_ctx.provider_display_names.first().cloned()
}
