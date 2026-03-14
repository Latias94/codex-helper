use super::stations_detail_controls::refresh_runtime_snapshot;
use super::*;

pub(super) fn render_station_runtime_control_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    snapshot: &GuiRuntimeSnapshot,
) {
    ui.label(pick(ctx.lang, "运行时控制", "Runtime control"));
    if snapshot.supports_station_runtime_override {
        let mut runtime_state = cfg.runtime_state;
        ui.horizontal(|ui| {
            ui.label(pick(ctx.lang, "状态", "State"));
            egui::ComboBox::from_id_salt(("stations_runtime_state", cfg.name.as_str()))
                .selected_text(runtime_config_state_label(ctx.lang, runtime_state))
                .show_ui(ui, |ui| {
                    for candidate in [
                        RuntimeConfigState::Normal,
                        RuntimeConfigState::Draining,
                        RuntimeConfigState::BreakerOpen,
                        RuntimeConfigState::HalfOpen,
                    ] {
                        ui.selectable_value(
                            &mut runtime_state,
                            candidate,
                            runtime_config_state_label(ctx.lang, candidate),
                        );
                    }
                });
            if runtime_state != cfg.runtime_state {
                match ctx.proxy.set_runtime_station_meta(
                    ctx.rt,
                    cfg.name.clone(),
                    None,
                    None,
                    Some(Some(runtime_state)),
                ) {
                    Ok(()) => {
                        refresh_runtime_snapshot(ctx);
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已应用站点运行时状态",
                                "Runtime station state updated",
                            )
                            .to_string(),
                        );
                    }
                    Err(error) => {
                        *ctx.last_error = Some(format!("apply runtime state failed: {error}"));
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            let mut enabled = cfg.enabled;
            if ui
                .checkbox(&mut enabled, pick(ctx.lang, "启用", "Enabled"))
                .changed()
            {
                match ctx.proxy.set_runtime_station_meta(
                    ctx.rt,
                    cfg.name.clone(),
                    Some(Some(enabled)),
                    None,
                    None,
                ) {
                    Ok(()) => {
                        refresh_runtime_snapshot(ctx);
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已应用站点运行时开关",
                                "Runtime station enabled updated",
                            )
                            .to_string(),
                        );
                    }
                    Err(error) => {
                        *ctx.last_error = Some(format!("apply runtime enabled failed: {error}"));
                    }
                }
            }

            let mut level = cfg.level.clamp(1, 10);
            ui.label(pick(ctx.lang, "等级", "Level"));
            egui::ComboBox::from_id_salt(("stations_runtime_level", cfg.name.as_str()))
                .selected_text(level.to_string())
                .show_ui(ui, |ui| {
                    for candidate in 1u8..=10 {
                        ui.selectable_value(&mut level, candidate, candidate.to_string());
                    }
                });
            if level != cfg.level {
                match ctx.proxy.set_runtime_station_meta(
                    ctx.rt,
                    cfg.name.clone(),
                    None,
                    Some(Some(level)),
                    None,
                ) {
                    Ok(()) => {
                        refresh_runtime_snapshot(ctx);
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已应用站点运行时等级",
                                "Runtime station level updated",
                            )
                            .to_string(),
                        );
                    }
                    Err(error) => {
                        *ctx.last_error = Some(format!("apply runtime level failed: {error}"));
                    }
                }
            }
        });

        let has_override = cfg.runtime_enabled_override.is_some()
            || cfg.runtime_level_override.is_some()
            || cfg.runtime_state_override.is_some();
        if ui
            .add_enabled(
                has_override,
                egui::Button::new(pick(ctx.lang, "清除运行时覆盖", "Clear runtime override")),
            )
            .clicked()
        {
            match ctx.proxy.set_runtime_station_meta(
                ctx.rt,
                cfg.name.clone(),
                cfg.runtime_enabled_override.map(|_| None),
                cfg.runtime_level_override.map(|_| None),
                cfg.runtime_state_override.map(|_| None),
            ) {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已清除站点运行时覆盖",
                            "Runtime station override cleared",
                        )
                        .to_string(),
                    );
                }
                Err(error) => {
                    *ctx.last_error =
                        Some(format!("clear runtime station override failed: {error}"));
                }
            }
        }
    } else {
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            pick(
                ctx.lang,
                "当前代理不支持运行时站点控制；此区域只读。",
                "This proxy does not support runtime station control; this area is read-only.",
            ),
        );
    }
}
