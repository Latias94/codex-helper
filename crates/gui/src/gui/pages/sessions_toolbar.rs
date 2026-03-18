use super::*;

pub(super) fn render_default_profile_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    profiles: &[ControlProfileOption],
    default_profile: Option<&str>,
) -> bool {
    if profiles.is_empty() {
        return false;
    }

    let mut force_refresh = false;
    let current_default_label = match default_profile {
        Some(name) => {
            format_profile_display(name, profiles.iter().find(|profile| profile.name == name))
        }
        None => pick(ctx.lang, "<无>", "<none>").to_string(),
    };

    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(pick(ctx.lang, "新会话默认 profile", "New-session default"));
            ui.monospace(current_default_label);

            let mut selected_default = ctx.view.sessions.default_profile_selection.clone();
            egui::ComboBox::from_id_salt("sessions_default_profile")
                .selected_text(match selected_default.as_deref() {
                    Some(name) => format_profile_display(
                        name,
                        profiles.iter().find(|profile| profile.name == name),
                    ),
                    None => pick(ctx.lang, "<选择>", "<select>").to_string(),
                })
                .show_ui(ui, |ui| {
                    for profile in profiles {
                        ui.selectable_value(
                            &mut selected_default,
                            Some(profile.name.clone()),
                            format_profile_display(profile.name.as_str(), Some(profile)),
                        );
                    }
                });
            if selected_default != ctx.view.sessions.default_profile_selection {
                ctx.view.sessions.default_profile_selection = selected_default;
            }

            if ui
                .button(pick(ctx.lang, "设为默认", "Set default"))
                .clicked()
            {
                if !snapshot.supports_default_profile_override {
                    *ctx.last_error = Some(
                        pick(
                            ctx.lang,
                            "当前代理不支持运行时切换默认 profile。",
                            "Current proxy does not support runtime default profile switch.",
                        )
                        .to_string(),
                    );
                } else if let Some(profile_name) =
                    ctx.view.sessions.default_profile_selection.clone()
                {
                    match ctx
                        .proxy
                        .set_default_profile(ctx.rt, Some(profile_name.clone()))
                    {
                        Ok(()) => {
                            force_refresh = true;
                            ctx.view.sessions.default_profile_selection = Some(profile_name);
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已切换新会话默认 profile",
                                    "Default profile switched",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("set default profile failed: {e}"));
                        }
                    }
                } else {
                    *ctx.last_error = Some(
                        pick(
                            ctx.lang,
                            "请先选择一个 profile。",
                            "Select a profile first.",
                        )
                        .to_string(),
                    );
                }
            }

            if ui
                .button(pick(ctx.lang, "回到持久化默认", "Use persisted default"))
                .clicked()
            {
                if !snapshot.supports_default_profile_override {
                    *ctx.last_error = Some(
                        pick(
                            ctx.lang,
                            "当前代理不支持运行时切换默认 profile。",
                            "Current proxy does not support runtime default profile switch.",
                        )
                        .to_string(),
                    );
                } else {
                    match ctx.proxy.set_default_profile(ctx.rt, None) {
                        Ok(()) => {
                            force_refresh = true;
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已恢复持久化默认 profile",
                                    "Fell back to persisted default profile",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("clear default profile failed: {e}"));
                        }
                    }
                }
            }
        });

        ui.small(pick(
            ctx.lang,
            "只影响新的 session；已经建立 binding 的会话会保持当前绑定。",
            "Only affects new sessions; already bound sessions keep their current binding.",
        ));
    });

    ui.add_space(6.0);
    force_refresh
}

pub(super) fn render_session_filter_controls(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut ctx.view.sessions.active_only,
            pick(ctx.lang, "仅活跃", "Active only"),
        );
        ui.checkbox(
            &mut ctx.view.sessions.errors_only,
            pick(ctx.lang, "仅错误", "Errors only"),
        );
        ui.checkbox(
            &mut ctx.view.sessions.overrides_only,
            pick(ctx.lang, "仅覆盖", "Overrides only"),
        );
        ui.checkbox(
            &mut ctx.view.sessions.lock_order,
            pick(ctx.lang, "锁定顺序", "Lock order"),
        )
        .on_hover_text(pick(
            ctx.lang,
            "暂停自动重排（活跃/最近分区与新会话插入也会暂停）",
            "Pause auto reordering (active partitioning and new-session insertion are paused too).",
        ));
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add_sized(
            [320.0, 20.0],
            egui::TextEdit::singleline(&mut ctx.view.sessions.search).hint_text(pick(
                ctx.lang,
                "按 session_id / cwd / model / station / profile 过滤…",
                "Filter by session_id / cwd / model / station / profile...",
            )),
        );
        if ui.button(pick(ctx.lang, "清空", "Clear")).clicked() {
            ctx.view.sessions.search.clear();
        }
    });

    ui.add_space(6.0);
}
