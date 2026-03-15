use super::*;

pub(super) fn render_session_station_override_editor(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    sid: &str,
    cfg_options: &[(String, String)],
) {
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "固定站点", "Pinned station"));

        let mut selected_name = ctx.view.sessions.editor.config_override.clone();
        egui::ComboBox::from_id_salt(("session_cfg_override", sid))
            .selected_text(match selected_name.as_deref() {
                Some(value) => value.to_string(),
                None => pick(ctx.lang, "<自动>", "<auto>").to_string(),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_name, None, pick(ctx.lang, "<自动>", "<auto>"));
                for (name, label) in cfg_options {
                    ui.selectable_value(&mut selected_name, Some(name.clone()), label);
                }
            });
        if selected_name != ctx.view.sessions.editor.config_override {
            ctx.view.sessions.editor.config_override = selected_name;
        }

        if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
            let desired = ctx.view.sessions.editor.config_override.clone();
            if !snapshot.supports_v1 {
                *ctx.last_error = Some(
                    pick(
                        ctx.lang,
                        "附着到的代理不支持会话固定站点（需要 API v1）。",
                        "Attached proxy does not support pinned session station (need API v1).",
                    )
                    .to_string(),
                );
            } else {
                match ctx
                    .proxy
                    .apply_session_station_override(ctx.rt, sid.to_string(), desired)
                {
                    Ok(()) => {
                        super::sessions_override_editors::refresh_runtime_snapshot(ctx);
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                    }
                    Err(error) => {
                        *ctx.last_error = Some(format!("apply override failed: {error}"));
                    }
                }
            }
        }
    });
}
