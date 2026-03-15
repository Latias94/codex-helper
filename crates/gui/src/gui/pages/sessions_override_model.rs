use super::*;

pub(super) fn render_session_model_override_editor(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    sid: &str,
) {
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "模型覆盖", "Model override"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.sessions.editor.model_override)
                .desired_width(180.0)
                .hint_text(pick(ctx.lang, "留空表示自动", "empty = auto")),
        );

        if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
            let desired = {
                let value = ctx.view.sessions.editor.model_override.trim().to_string();
                if value.is_empty() { None } else { Some(value) }
            };
            if !snapshot.supports_v1 {
                *ctx.last_error = Some(
                    pick(
                        ctx.lang,
                        "附着到的代理不支持会话模型覆盖（需要 API v1）。",
                        "Attached proxy does not support session model override (need API v1).",
                    )
                    .to_string(),
                );
            } else {
                match ctx
                    .proxy
                    .apply_session_model_override(ctx.rt, sid.to_string(), desired)
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
