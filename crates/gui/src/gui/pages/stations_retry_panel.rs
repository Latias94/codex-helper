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

            let base_retry = snapshot.configured_retry.as_ref().cloned().unwrap_or_default();
            let draft_retry = build_retry_config_from_editor(
                &ctx.view.stations.retry_editor,
                &base_retry,
            );
            render_retry_draft_preview(ui, ctx.lang, draft_retry.as_ref());

            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "写回 retry 配置", "Apply persisted retry config"))
                    .clicked()
                {
                    match draft_retry.clone() {
                        Ok(retry) => match ctx.proxy.set_persisted_retry_config(ctx.rt, retry) {
                            Ok(()) => {
                                ctx.proxy.refresh_current_if_due(
                                    ctx.rt,
                                    std::time::Duration::from_secs(0),
                                );
                                refresh_proxy_settings_editor_from_disk_if_running(ctx);
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
            ui.small(retry_policy_preview_text(
                ctx.lang,
                snapshot
                    .configured_retry
                    .as_ref()
                    .and_then(|retry| retry.profile),
                retry,
            ));
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

fn render_retry_draft_preview(
    ui: &mut egui::Ui,
    lang: Language,
    draft_retry: Result<&RetryConfig, &String>,
) {
    ui.add_space(4.0);
    ui.label(pick(lang, "草稿预览", "Draft preview"));
    match draft_retry {
        Ok(retry) => {
            let resolved = retry.resolve();
            ui.small(retry_policy_preview_text(lang, retry.profile, &resolved));
            ui.colored_label(
                retry_policy_risk_color(&resolved),
                retry_policy_risk_text(lang, &resolved),
            );
        }
        Err(err) => {
            ui.colored_label(
                egui::Color32::from_rgb(190, 80, 80),
                format!("invalid draft: {err}"),
            );
        }
    }
}

fn retry_policy_preview_text(
    lang: Language,
    configured_profile: Option<RetryProfileName>,
    retry: &crate::config::ResolvedRetryConfig,
) -> String {
    let profile = configured_profile
        .map(|profile| retry_profile_name_value(profile).to_string())
        .unwrap_or_else(|| pick(lang, "auto/balanced", "auto/balanced").to_string());
    let cross_station = if retry.allow_cross_station_before_first_output {
        pick(lang, "允许", "allowed")
    } else {
        pick(lang, "关闭", "disabled")
    };
    format!(
        "{}: profile={} | upstream {} x{} | provider {} x{} | {}={}",
        pick(lang, "预览", "Preview"),
        profile,
        retry_strategy_label(retry.upstream.strategy),
        retry.upstream.max_attempts,
        retry_strategy_label(retry.provider.strategy),
        retry.provider.max_attempts,
        pick(lang, "首包前跨站", "cross-station before first output"),
        cross_station
    )
}

fn retry_policy_risk_text(
    lang: Language,
    retry: &crate::config::ResolvedRetryConfig,
) -> &'static str {
    if retry.allow_cross_station_before_first_output {
        pick(
            lang,
            "风险提示：首包前可跨 station failover；首包后仍会锁定已提交路线。",
            "Risk: cross-station failover is allowed before first output; after first output the committed route remains sticky.",
        )
    } else {
        pick(
            lang,
            "安全边界：自动 retry 保持在同 station / upstream 策略内，不会主动跨 station。",
            "Boundary: automatic retry stays within the same-station/upstream policy and will not proactively cross stations.",
        )
    }
}

fn retry_policy_risk_color(retry: &crate::config::ResolvedRetryConfig) -> egui::Color32 {
    if retry.allow_cross_station_before_first_output {
        egui::Color32::from_rgb(200, 120, 40)
    } else {
        egui::Color32::from_rgb(100, 150, 110)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_policy_preview_mentions_cross_station_boundary() {
        let retry = RetryProfileName::AggressiveFailover.defaults();

        let text = retry_policy_preview_text(
            Language::En,
            Some(RetryProfileName::AggressiveFailover),
            &retry,
        );

        assert!(text.contains("profile=aggressive-failover"));
        assert!(text.contains("cross-station before first output=allowed"));
        assert!(text.contains("provider failover x3"));
    }

    #[test]
    fn retry_policy_preview_marks_default_balanced_when_profile_missing() {
        let retry = RetryProfileName::Balanced.defaults();

        let text = retry_policy_preview_text(Language::En, None, &retry);

        assert!(text.contains("profile=auto/balanced"));
        assert!(text.contains("cross-station before first output=disabled"));
    }

    #[test]
    fn retry_policy_risk_text_warns_for_cross_station_failover() {
        let retry = RetryProfileName::CostPrimary.defaults();

        let text = retry_policy_risk_text(Language::En, &retry);

        assert!(text.contains("Risk: cross-station failover is allowed"));
        assert!(text.contains("after first output"));
    }

    #[test]
    fn retry_policy_risk_text_marks_same_station_boundary() {
        let retry = RetryProfileName::Balanced.defaults();

        let text = retry_policy_risk_text(Language::En, &retry);

        assert!(text.contains("Boundary: automatic retry stays"));
        assert!(text.contains("will not proactively cross stations"));
    }
}
