use super::sessions_override_editors::render_session_override_editors;
use super::sessions_profile_binding_editor::render_session_profile_binding_editor;
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_session_detail_controls(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    row: &SessionRow,
    profiles: &[ControlProfileOption],
    global_station_override: Option<&str>,
    action_apply_session_profile: &mut Option<(String, String)>,
    action_clear_session_profile_binding: &mut Option<String>,
    action_clear_session_manual_overrides: &mut Option<String>,
) {
    let override_model = row.override_model.as_deref().unwrap_or("-");
    let override_cfg = row.override_station_name().unwrap_or("-");
    let override_eff = row.override_effort.as_deref().unwrap_or("-");
    let override_service_tier = row.override_service_tier.as_deref().unwrap_or("-");
    let global_cfg = global_station_override.unwrap_or("-");
    ui.label(format!(
        "{}: model={override_model}, effort={override_eff}, station={override_cfg}, tier={override_service_tier}, global_station={global_cfg}",
        pick(ctx.lang, "覆盖", "Overrides")
    ));

    let Some(sid) = row.session_id.clone() else {
        ui.label(pick(
            ctx.lang,
            "该条目没有 session_id，暂不支持编辑覆盖。",
            "This entry has no session_id; overrides editing is disabled.",
        ));
        return;
    };

    let cfg_options = station_options_from_gui_stations(&snapshot.stations);
    let has_session_manual_overrides = row.override_model.is_some()
        || row.override_station_name().is_some()
        || row.override_effort.is_some()
        || row.override_service_tier.is_some();

    ui.add_space(6.0);
    ui.separator();
    render_last_route_decision_card(ui, ctx.lang, row);

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "会话覆盖设置", "Session overrides"));
        let reset_overrides = ui
            .add_enabled(
                snapshot.supports_session_override_reset && has_session_manual_overrides,
                egui::Button::new(pick(
                    ctx.lang,
                    "重置 manual overrides",
                    "Reset manual overrides",
                )),
            )
            .on_hover_text(pick(
                ctx.lang,
                "清除当前会话的 model / station / effort / service_tier 覆盖，不影响已绑定的 profile。",
                "Clear the current session model / station / effort / service_tier overrides without touching the bound profile.",
            ));
        if reset_overrides.clicked() {
            *action_clear_session_manual_overrides = Some(sid.clone());
        }
    });

    render_session_profile_binding_editor(
        ui,
        ctx,
        row,
        sid.as_str(),
        profiles,
        action_apply_session_profile,
        action_clear_session_profile_binding,
    );

    render_session_override_editors(ui, ctx, snapshot, sid.as_str(), &cfg_options);
}
