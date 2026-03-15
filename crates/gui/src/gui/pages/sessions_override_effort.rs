use super::*;

pub(super) fn render_session_effort_override_editor(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    sid: &str,
) {
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "推理强度", "Reasoning effort"));

        let mut choice = match ctx.view.sessions.editor.effort_override.as_deref() {
            None => "auto",
            Some("low") => "low",
            Some("medium") => "medium",
            Some("high") => "high",
            Some("xhigh") => "xhigh",
            Some(_) => "custom",
        };

        egui::ComboBox::from_id_salt(("session_effort_choice", sid))
            .selected_text(choice)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, "auto", "auto");
                ui.selectable_value(&mut choice, "low", "low");
                ui.selectable_value(&mut choice, "medium", "medium");
                ui.selectable_value(&mut choice, "high", "high");
                ui.selectable_value(&mut choice, "xhigh", "xhigh");
                ui.selectable_value(&mut choice, "custom", "custom");
            });

        if choice == "auto" {
            ctx.view.sessions.editor.effort_override = None;
        } else if choice != "custom" {
            ctx.view.sessions.editor.effort_override = Some(choice.to_string());
            ctx.view.sessions.editor.custom_effort = choice.to_string();
        } else if ctx.view.sessions.editor.effort_override.is_none() {
            ctx.view.sessions.editor.effort_override =
                Some(ctx.view.sessions.editor.custom_effort.clone());
        }

        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.sessions.editor.custom_effort)
                .desired_width(90.0)
                .hint_text(pick(ctx.lang, "自定义", "custom")),
        );

        if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
            let desired = match choice {
                "auto" => None,
                "custom" => {
                    let value = ctx.view.sessions.editor.custom_effort.trim().to_string();
                    if value.is_empty() { None } else { Some(value) }
                }
                value => Some(value.to_string()),
            };
            match ctx
                .proxy
                .apply_session_effort_override(ctx.rt, sid.to_string(), desired)
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
    });
}
