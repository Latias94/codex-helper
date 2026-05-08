use super::session_control_posture::session_has_manual_overrides;
use super::*;

pub(super) fn format_profile_display(name: &str, profile: Option<&ControlProfileOption>) -> String {
    match profile {
        Some(profile) if profile.is_default => format!("{name} [default]"),
        _ => name.to_string(),
    }
}

pub(super) fn format_profile_summary(profile: &ControlProfileOption) -> String {
    let extends = profile.extends.as_deref().unwrap_or("<none>");
    let model = profile.model.as_deref().unwrap_or("auto");
    let effort = profile.reasoning_effort.as_deref().unwrap_or("auto");
    let tier = profile.service_tier.as_deref().unwrap_or("auto");
    format!("extends={extends}, model={model}, effort={effort}, tier={tier}")
}

pub(super) fn format_service_profile_summary(
    profile: &crate::config::ServiceControlProfile,
) -> String {
    let extends = profile.extends.as_deref().unwrap_or("<none>");
    let model = profile.model.as_deref().unwrap_or("auto");
    let effort = profile.reasoning_effort.as_deref().unwrap_or("auto");
    let tier = profile.service_tier.as_deref().unwrap_or("auto");
    format!("extends={extends}, model={model}, effort={effort}, tier={tier}")
}

pub(super) fn service_profile_from_option(
    profile: &ControlProfileOption,
) -> crate::config::ServiceControlProfile {
    crate::config::ServiceControlProfile {
        extends: profile.extends.clone(),
        station: None,
        model: profile.model.clone(),
        reasoning_effort: profile.reasoning_effort.clone(),
        service_tier: profile.service_tier.clone(),
    }
}

pub(super) fn resolve_service_profile_from_options(
    profile_name: &str,
    profiles: &[ControlProfileOption],
) -> anyhow::Result<crate::config::ServiceControlProfile> {
    let profile_catalog = profiles
        .iter()
        .map(|profile| (profile.name.clone(), service_profile_from_option(profile)))
        .collect::<BTreeMap<_, _>>();
    crate::config::resolve_service_profile_from_catalog(&profile_catalog, profile_name)
}

pub(super) fn session_binding_profile_summary(
    row: &SessionRow,
    profiles: &[ControlProfileOption],
    lang: Language,
) -> Option<String> {
    let profile_name = row.binding_profile_name.as_deref()?;
    let raw_profile = profiles.iter().find(|profile| profile.name == profile_name);
    if raw_profile.is_none() {
        return Some(
            pick(
                lang,
                "当前工作台中已缺失该 profile",
                "This profile is missing from the current workspace",
            )
            .to_string(),
        );
    }

    match resolve_service_profile_from_options(profile_name, profiles) {
        Ok(profile) => Some(format_service_profile_summary(&profile)),
        Err(_) => raw_profile.map(format_profile_summary),
    }
}

pub(super) fn render_session_profile_apply_preview(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    profile_name: &str,
    profile: &crate::config::ServiceControlProfile,
) {
    let has_manual_overrides = session_has_manual_overrides(row);

    ui.add_space(6.0);
    ui.group(|ui| {
        ui.label(pick(lang, "应用预览", "Apply preview"));
        ui.small(pick(
            lang,
            "应用 profile 会重写当前 session binding，并清空当前会话的手动覆盖（包括 station / model / reasoning / service_tier）。",
            "Applying a profile rewrites the current session binding and clears the session's manual overrides, including station / model / reasoning / service_tier.",
        ));

        if row.binding_profile_name.as_deref() == Some(profile_name) {
            ui.small(if has_manual_overrides {
                pick(
                    lang,
                    "该会话已经绑定到这个 profile，但重新应用仍会清空手动 session overrides。",
                    "This session is already bound to this profile, but reapplying it will still clear manual session overrides.",
                )
            } else {
                pick(
                    lang,
                    "该会话已经绑定到这个 profile；重新应用通常只会刷新同一份绑定。",
                    "This session is already bound to this profile; reapplying it usually just refreshes the same binding.",
                )
            });
        }

        ui.small(format!(
            "{}: {} -> {}",
            pick(lang, "binding profile", "binding profile"),
            row.binding_profile_name
                .as_deref()
                .unwrap_or_else(|| pick(lang, "<无>", "<none>")),
            profile_name
        ));
        ui.small(format!(
            "model: {} -> {}",
            session_route_preview_value(row.effective_model.as_ref(), row.last_model.as_deref(), lang),
            session_profile_target_value(profile.model.as_deref(), lang)
        ));
        ui.small(format!(
            "reasoning: {} -> {}",
            session_route_preview_value(
                row.effective_reasoning_effort.as_ref(),
                row.last_reasoning_effort.as_deref(),
                lang,
            ),
            session_profile_target_value(profile.reasoning_effort.as_deref(), lang)
        ));
        ui.small(format!(
            "service_tier: {} -> {}",
            session_route_preview_value(
                row.effective_service_tier.as_ref(),
                row.last_service_tier.as_deref(),
                lang,
            ),
            session_profile_target_value(profile.service_tier.as_deref(), lang)
        ));
    });
}
