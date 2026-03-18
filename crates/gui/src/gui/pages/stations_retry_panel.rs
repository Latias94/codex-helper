use super::*;

pub(super) fn render_retry_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
) {
    if snapshot.supports_retry_config_api {
        let configured_retry = snapshot.configured_retry.clone().unwrap_or_default();
        sync_stations_retry_editor(&mut ctx.view.stations.retry_editor, &configured_retry);
    }

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "Retry / Failover", "Retry / Failover"));
        ui.label(pick(
            ctx.lang,
            "这里管理全局的 retry profile 与冷却/熔断惩罚；它影响整个代理的路由行为，不是单个 station 的局部设置。",
            "Manage the global retry profile plus cooldown/breaker penalties here; it affects whole-proxy routing behavior rather than a single station.",
        ));

        if snapshot.supports_retry_config_api {
            {
                let editor = &mut ctx.view.stations.retry_editor;
                ui.horizontal(|ui| {
                    ui.label(pick(ctx.lang, "Retry profile", "Retry profile"));
                    egui::ComboBox::from_id_salt("stations_retry_profile")
                        .selected_text(retry_profile_display_text(
                            ctx.lang,
                            retry_profile_name_from_value(editor.profile.as_str()),
                        ))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut editor.profile,
                                String::new(),
                                retry_profile_display_text(ctx.lang, None),
                            );
                            for profile in [
                                RetryProfileName::Balanced,
                                RetryProfileName::SameUpstream,
                                RetryProfileName::AggressiveFailover,
                                RetryProfileName::CostPrimary,
                            ] {
                                ui.selectable_value(
                                    &mut editor.profile,
                                    retry_profile_name_value(profile).to_string(),
                                    retry_profile_display_text(ctx.lang, Some(profile)),
                                );
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("cf challenge");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cloudflare_challenge_cooldown_secs),
                    );
                    ui.label("cf timeout");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cloudflare_timeout_cooldown_secs),
                    );
                    ui.label("transport");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.transport_cooldown_secs),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("backoff factor");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cooldown_backoff_factor),
                    );
                    ui.label("backoff max");
                    ui.add_sized(
                        [72.0, 22.0],
                        egui::TextEdit::singleline(&mut editor.cooldown_backoff_max_secs),
                    );
                });
            }

            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "写回 retry 配置", "Apply persisted retry config"))
                    .clicked()
                {
                    let base_retry = snapshot.configured_retry.as_ref().cloned().unwrap_or_default();
                    match build_retry_config_from_editor(&ctx.view.stations.retry_editor, &base_retry)
                    {
                        Ok(retry) => match ctx.proxy.set_persisted_retry_config(ctx.rt, retry) {
                            Ok(()) => {
                                ctx.proxy.refresh_current_if_due(
                                    ctx.rt,
                                    std::time::Duration::from_secs(0),
                                );
                                refresh_config_editor_from_disk_if_running(ctx);
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已写回 retry/failover 配置",
                                        "Persisted retry/failover config updated",
                                    )
                                    .to_string(),
                                );
                                *ctx.last_error = None;
                            }
                            Err(e) => {
                                *ctx.last_error =
                                    Some(format!("set persisted retry config failed: {e}"));
                            }
                        },
                        Err(e) => {
                            *ctx.last_error = Some(format!("invalid retry config: {e}"));
                        }
                    }
                }

                if ui
                    .button(pick(ctx.lang, "恢复 balanced 表单", "Reset form to balanced"))
                    .clicked()
                {
                    load_stations_retry_editor_fields(
                        &mut ctx.view.stations.retry_editor,
                        &RetryConfig::default(),
                    );
                }
            });

            ui.small(if matches!(snapshot.kind, ProxyModeKind::Attached) {
                pick(
                    ctx.lang,
                    "附着模式下，这里直接写回远端代理暴露的 retry config API，不依赖本机文件。",
                    "In attached mode, this writes directly to the remote proxy's retry config API instead of any local file on this device.",
                )
            } else {
                pick(
                    ctx.lang,
                    "本地运行模式下，这里通过 control-plane 写回持久化 retry 策略并触发 reload。",
                    "In local running mode, this writes through the control plane to persisted retry policy and reloads the runtime.",
                )
            });
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                if matches!(snapshot.kind, ProxyModeKind::Attached) {
                    pick(
                        ctx.lang,
                        "当前附着目标没有暴露 retry config API，因此这里只能查看 resolved policy，不能直接写回。",
                        "This attached target does not expose retry config APIs, so only the resolved policy is visible here.",
                    )
                } else {
                    pick(
                        ctx.lang,
                        "当前运行态没有可写 retry config API；下面仅展示 resolved policy。",
                        "No writable retry config API is available for the current runtime; only the resolved policy is shown below.",
                    )
                },
            );
        }

        ui.add_space(6.0);
        ui.separator();
        ui.label(pick(ctx.lang, "Resolved policy", "Resolved policy"));
        if let Some(retry) = snapshot.resolved_retry.as_ref() {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "upstream: {} / attempts={}",
                    retry_strategy_label(retry.upstream.strategy),
                    retry.upstream.max_attempts
                ));
                ui.label(format!(
                    "provider: {} / attempts={}",
                    retry_strategy_label(retry.provider.strategy),
                    retry.provider.max_attempts
                ));
            });
            ui.horizontal(|ui| {
                ui.label(format!(
                    "cf challenge={}s",
                    retry.cloudflare_challenge_cooldown_secs
                ));
                ui.label(format!(
                    "cf timeout={}s",
                    retry.cloudflare_timeout_cooldown_secs
                ));
                ui.label(format!("transport={}s", retry.transport_cooldown_secs));
            });
            ui.horizontal(|ui| {
                ui.label(format!(
                    "backoff factor={}",
                    retry.cooldown_backoff_factor
                ));
                ui.label(format!(
                    "backoff max={}s",
                    retry.cooldown_backoff_max_secs
                ));
            });
            ui.small(format!(
                "upstream backoff={}..{} ms  provider backoff={}..{} ms",
                retry.upstream.backoff_ms,
                retry.upstream.backoff_max_ms,
                retry.provider.backoff_ms,
                retry.provider.backoff_max_ms
            ));
            ui.small(pick(
                ctx.lang,
                "同站点 failover 规则：优先在当前 station 内尝试其他 eligible upstream，只有当前 station 耗尽后才会考虑下一个 station。",
                "Same-station failover rule: exhaust other eligible upstreams inside the current station before considering the next station.",
            ));
        } else {
            ui.label(pick(
                ctx.lang,
                "当前还没有可见的 resolved retry policy。",
                "No resolved retry policy is visible for the current runtime yet.",
            ));
        }
    });
}
