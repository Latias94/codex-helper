use super::stations_detail_controls::refresh_runtime_snapshot;
use super::*;

pub(super) fn render_station_persisted_settings_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    snapshot: &GuiRuntimeSnapshot,
    configured_active_station: Option<&str>,
    supports_persisted_station_settings: bool,
) {
    ui.label(pick(
        ctx.lang,
        "持久化站点设置",
        "Persisted station settings",
    ));
    if supports_persisted_station_settings {
        ui.horizontal(|ui| {
            if ui
                .button(pick(
                    ctx.lang,
                    "设为持久化 active_station",
                    "Set persisted active_station",
                ))
                .clicked()
            {
                match ctx
                    .proxy
                    .set_persisted_active_station(ctx.rt, Some(cfg.name.clone()))
                {
                    Ok(()) => {
                        refresh_runtime_snapshot(ctx);
                        refresh_config_editor_from_disk_if_running(ctx);
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已更新持久化 active_station",
                                "Persisted active_station updated",
                            )
                            .to_string(),
                        );
                        *ctx.last_error = None;
                    }
                    Err(error) => {
                        *ctx.last_error =
                            Some(format!("set persisted active station failed: {error}"));
                    }
                }
            }
            if ui
                .add_enabled(
                    configured_active_station.is_some(),
                    egui::Button::new(pick(
                        ctx.lang,
                        "清除持久化 active_station",
                        "Clear persisted active_station",
                    )),
                )
                .clicked()
            {
                match ctx.proxy.set_persisted_active_station(ctx.rt, None) {
                    Ok(()) => {
                        refresh_runtime_snapshot(ctx);
                        refresh_config_editor_from_disk_if_running(ctx);
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已清除持久化 active_station",
                                "Persisted active_station cleared",
                            )
                            .to_string(),
                        );
                        *ctx.last_error = None;
                    }
                    Err(error) => {
                        *ctx.last_error =
                            Some(format!("clear persisted active station failed: {error}"));
                    }
                }
            }
        });

        let mut persisted_enabled = cfg.configured_enabled;
        let mut persisted_level = cfg.configured_level.clamp(1, 10);
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut persisted_enabled,
                pick(ctx.lang, "持久化启用", "Persisted enabled"),
            );
            ui.label(pick(ctx.lang, "持久化等级", "Persisted level"));
            egui::ComboBox::from_id_salt(("stations_persisted_level", cfg.name.as_str()))
                .selected_text(persisted_level.to_string())
                .show_ui(ui, |ui| {
                    for candidate in 1u8..=10 {
                        ui.selectable_value(&mut persisted_level, candidate, candidate.to_string());
                    }
                });
        });
        if persisted_enabled != cfg.configured_enabled
            || persisted_level != cfg.configured_level.clamp(1, 10)
        {
            match ctx.proxy.update_persisted_station(
                ctx.rt,
                cfg.name.clone(),
                persisted_enabled,
                persisted_level,
            ) {
                Ok(()) => {
                    refresh_runtime_snapshot(ctx);
                    refresh_config_editor_from_disk_if_running(ctx);
                    *ctx.last_info = Some(
                        pick(
                            ctx.lang,
                            "已写回持久化站点字段",
                            "Persisted station fields updated",
                        )
                        .to_string(),
                    );
                    *ctx.last_error = None;
                }
                Err(error) => {
                    *ctx.last_error =
                        Some(format!("update persisted station fields failed: {error}"));
                }
            }
        }
        ui.small(if matches!(snapshot.kind, ProxyModeKind::Attached) {
            pick(
                ctx.lang,
                "这里直接写回附着代理的持久化站点设置，不依赖本机文件。",
                "These controls write back to the attached proxy's persisted station settings directly and do not rely on this device's local file.",
            )
        } else {
            pick(
                ctx.lang,
                "这里通过本地 control-plane 写回持久化站点设置，并与运行态保持同步。",
                "These controls write back through the local control plane to persisted station settings and keep runtime in sync.",
            )
        });
    } else {
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            pick(
                ctx.lang,
                "当前目标没有暴露持久化站点设置 API，因此这里只能查看持久化设置，不能直接修改。",
                "This target does not expose persisted station settings APIs yet, so persisted fields are view-only here.",
            ),
        );
    }
}
