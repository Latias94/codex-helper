use super::sessions_controller_types::SessionPageActions;
use super::*;

pub(super) fn apply_session_page_actions(
    ctx: &mut PageCtx<'_>,
    actions: SessionPageActions,
    default_profile: Option<&str>,
    profiles: &[ControlProfileOption],
) -> bool {
    let mut force_refresh = false;

    if let Some((sid, profile_name)) = actions.apply_session_profile {
        match ctx
            .proxy
            .apply_session_profile(ctx.rt, sid, profile_name.clone())
        {
            Ok(()) => {
                force_refresh = true;
                *ctx.last_info = Some(format!(
                    "{}: {profile_name}",
                    pick(ctx.lang, "已应用 profile", "Profile applied")
                ));
            }
            Err(e) => {
                *ctx.last_error = Some(format!("apply profile failed: {e}"));
            }
        }
    }

    if let Some(sid) = actions.clear_session_manual_overrides {
        match ctx.proxy.clear_session_manual_overrides(ctx.rt, sid) {
            Ok(()) => {
                force_refresh = true;
                ctx.view.sessions.editor.model_override.clear();
                ctx.view.sessions.editor.config_override = None;
                ctx.view.sessions.editor.effort_override = None;
                ctx.view.sessions.editor.custom_effort.clear();
                ctx.view.sessions.editor.service_tier_override = None;
                ctx.view.sessions.editor.custom_service_tier.clear();
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已重置 session manual overrides",
                        "Session manual overrides reset",
                    )
                    .to_string(),
                );
            }
            Err(e) => {
                *ctx.last_error = Some(format!("reset session manual overrides failed: {e}"));
            }
        }
    }

    if let Some(sid) = actions.clear_session_profile_binding {
        match ctx.proxy.clear_session_profile_binding(ctx.rt, sid) {
            Ok(()) => {
                force_refresh = true;
                ctx.view.sessions.editor.profile_selection = default_profile
                    .map(ToOwned::to_owned)
                    .or_else(|| profiles.first().map(|profile| profile.name.clone()));
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已清除 profile binding",
                        "Profile binding cleared",
                    )
                    .to_string(),
                );
            }
            Err(e) => {
                *ctx.last_error = Some(format!("clear profile binding failed: {e}"));
            }
        }
    }

    force_refresh
}
