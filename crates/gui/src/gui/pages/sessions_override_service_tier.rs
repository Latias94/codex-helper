use super::*;

pub(super) fn render_session_service_tier_override_editor(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    sid: &str,
) {
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "Fast / Service Tier", "Fast / Service tier"));

        let mut choice = match ctx.view.sessions.editor.service_tier_override.as_deref() {
            None => "auto",
            Some("default") => "default",
            Some("priority") => "priority",
            Some("flex") => "flex",
            Some(_) => "custom",
        };

        egui::ComboBox::from_id_salt(("session_service_tier_choice", sid))
            .selected_text(match choice {
                "priority" => pick(ctx.lang, "priority（fast）", "priority (fast)"),
                value => value,
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, "auto", "auto");
                ui.selectable_value(&mut choice, "default", "default");
                ui.selectable_value(
                    &mut choice,
                    "priority",
                    pick(ctx.lang, "priority（fast）", "priority (fast)"),
                );
                ui.selectable_value(&mut choice, "flex", "flex");
                ui.selectable_value(&mut choice, "custom", "custom");
            });

        if choice == "auto" {
            ctx.view.sessions.editor.service_tier_override = None;
        } else if choice != "custom" {
            ctx.view.sessions.editor.service_tier_override = Some(choice.to_string());
            ctx.view.sessions.editor.custom_service_tier = choice.to_string();
        } else if ctx.view.sessions.editor.service_tier_override.is_none() {
            ctx.view.sessions.editor.service_tier_override =
                Some(ctx.view.sessions.editor.custom_service_tier.clone());
        }

        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.sessions.editor.custom_service_tier)
                .desired_width(100.0)
                .hint_text(pick(ctx.lang, "自定义", "custom")),
        );

        if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
            let desired = match choice {
                "auto" => None,
                "custom" => {
                    let value = ctx
                        .view
                        .sessions
                        .editor
                        .custom_service_tier
                        .trim()
                        .to_string();
                    if value.is_empty() { None } else { Some(value) }
                }
                value => Some(value.to_string()),
            };
            if !snapshot.supports_v1 {
                *ctx.last_error = Some(
                    pick(
                        ctx.lang,
                        "附着到的代理不支持会话 service tier 覆盖（需要 API v1）。",
                        "Attached proxy does not support session service tier override (need API v1).",
                    )
                    .to_string(),
                );
            } else {
                match ctx
                    .proxy
                    .apply_session_service_tier_override(ctx.rt, sid.to_string(), desired)
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
