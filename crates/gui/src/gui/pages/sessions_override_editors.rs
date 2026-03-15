use super::*;

pub(super) fn render_session_override_editors(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    sid: &str,
    cfg_options: &[(String, String)],
) {
    super::sessions_override_model::render_session_model_override_editor(ui, ctx, snapshot, sid);
    super::sessions_override_station::render_session_station_override_editor(
        ui,
        ctx,
        snapshot,
        sid,
        cfg_options,
    );
    super::sessions_override_effort::render_session_effort_override_editor(ui, ctx, sid);
    super::sessions_override_service_tier::render_session_service_tier_override_editor(
        ui, ctx, snapshot, sid,
    );
}

pub(in crate::gui::pages) fn refresh_runtime_snapshot(ctx: &mut PageCtx<'_>) {
    ctx.proxy
        .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
}
