use super::*;

pub(super) fn render_session_override_editors(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    sid: &str,
    cfg_options: &[(String, String)],
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
                        refresh_runtime_snapshot(ctx);
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("apply override failed: {e}"));
                    }
                }
            }
        }
    });

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
                        refresh_runtime_snapshot(ctx);
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("apply override failed: {e}"));
                    }
                }
            }
        }
    });

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
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("apply override failed: {e}"));
                }
            }
        }
    });

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
                        refresh_runtime_snapshot(ctx);
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("apply override failed: {e}"));
                    }
                }
            }
        }
    });
}

fn refresh_runtime_snapshot(ctx: &mut PageCtx<'_>) {
    ctx.proxy
        .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
}
