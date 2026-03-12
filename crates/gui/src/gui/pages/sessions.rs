use super::components::console_layout::{
    ConsoleTone, console_kv_grid, console_note, console_section,
};
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "会话", "Sessions"));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "当前未运行代理，也未附着到现有代理。请在“总览”里启动或附着后再查看会话。",
            "No proxy is running or attached. Start or attach on Overview to view sessions.",
        ));
        return;
    };
    let host_local_session_features = host_local_session_features_available(ctx.proxy);

    let last_error = snapshot.last_error.clone();
    let active = snapshot.active.clone();
    let recent = snapshot.recent.clone();
    let global_station_override = snapshot.global_station_override.clone();
    let default_profile = snapshot.default_profile.clone();
    let profiles = snapshot.profiles.clone();
    let session_model_overrides = snapshot.session_model_overrides.clone();
    let session_effort_overrides = snapshot.session_effort_overrides.clone();
    let session_station_overrides = snapshot.session_station_overrides.clone();
    let session_service_tier_overrides = snapshot.session_service_tier_overrides.clone();
    let session_stats = snapshot.session_stats.clone();
    let configured_active_station = snapshot.configured_active_station.clone();
    let effective_active_station = snapshot.effective_active_station.clone();
    let mut force_refresh = false;
    let runtime_station_catalog = snapshot
        .stations
        .iter()
        .cloned()
        .map(|config| (config.name.clone(), config))
        .collect::<BTreeMap<_, _>>();
    let session_preview_service_name =
        snapshot
            .service_name
            .as_deref()
            .unwrap_or(match ctx.view.config.service {
                crate::config::ServiceKind::Claude => "claude",
                crate::config::ServiceKind::Codex => "codex",
            });
    let session_preview_catalogs = ctx
        .proxy
        .attached()
        .and_then(|att| {
            att.supports_station_spec_api.then(|| {
                (
                    att.persisted_stations.clone(),
                    att.persisted_station_providers.clone(),
                )
            })
        })
        .or_else(|| {
            if matches!(ctx.proxy.kind(), ProxyModeKind::Attached) {
                None
            } else {
                local_profile_preview_catalogs_from_text(
                    ctx.proxy_config_text,
                    session_preview_service_name,
                )
            }
        });
    let session_preview_station_specs = session_preview_catalogs
        .as_ref()
        .map(|(stations, _)| stations);
    let session_preview_provider_catalog = session_preview_catalogs
        .as_ref()
        .map(|(_, providers)| providers);
    let session_preview_runtime_station_catalog = Some(&runtime_station_catalog);

    if ctx
        .view
        .sessions
        .default_profile_selection
        .as_ref()
        .is_none_or(|name| !profiles.iter().any(|profile| profile.name == *name))
    {
        ctx.view.sessions.default_profile_selection = default_profile
            .clone()
            .or_else(|| profiles.first().map(|profile| profile.name.clone()));
    }

    if let Some(err) = last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        ui.add_space(4.0);
    }

    if remote_attached_proxy_active(ctx.proxy) {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            pick(
                ctx.lang,
                "当前附着的是远端代理：共享的 session 控制仍可用，但 cwd / transcript 这类 host-local 入口已按远端模式收敛。",
                "A remote proxy is attached: shared session controls remain available, but host-local entries such as cwd/transcript are gated for remote safety.",
            ),
        );
        ui.add_space(4.0);
    }

    if !profiles.is_empty() {
        let current_default_label = match default_profile.as_deref() {
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
                        for profile in profiles.iter() {
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
                    .button(pick(ctx.lang, "回到配置默认", "Use config default"))
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
                                        "已恢复配置文件默认 profile",
                                        "Fell back to config default profile",
                                    )
                                    .to_string(),
                                );
                            }
                            Err(e) => {
                                *ctx.last_error =
                                    Some(format!("clear default profile failed: {e}"));
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
    }

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
                "按 session_id / cwd / model / station / config 过滤…",
                "Filter by session_id / cwd / model / station / config...",
            )),
        );
        if ui.button(pick(ctx.lang, "清空", "Clear")).clicked() {
            ctx.view.sessions.search.clear();
        }
    });

    ui.add_space(6.0);

    let has_session_cards = !snapshot.session_cards.is_empty();
    let rows = if has_session_cards {
        build_session_rows_from_cards(&snapshot.session_cards)
    } else {
        build_session_rows(
            active,
            &recent,
            &session_model_overrides,
            &session_effort_overrides,
            &session_station_overrides,
            &session_service_tier_overrides,
            global_station_override.as_deref(),
            &session_stats,
        )
    };

    let mut row_index_by_id = HashMap::new();
    for (idx, row) in rows.iter().enumerate() {
        row_index_by_id.insert(row.session_id.clone(), idx);
    }

    sync_session_order(&mut ctx.view.sessions, &rows);

    let q = ctx.view.sessions.search.trim().to_lowercase();
    let filtered = ctx
        .view
        .sessions
        .ordered_session_ids
        .iter()
        .filter_map(|id| row_index_by_id.get(id).copied().map(|idx| &rows[idx]))
        .filter(|row| {
            if ctx.view.sessions.active_only && row.active_count == 0 {
                return false;
            }
            if ctx.view.sessions.errors_only && row.last_status.is_some_and(|s| s < 400) {
                return false;
            }
            if ctx.view.sessions.overrides_only
                && row.override_model.is_none()
                && row.override_effort.is_none()
                && row.override_station_name().is_none()
                && row.override_service_tier.is_none()
            {
                return false;
            }
            session_row_matches_query(row, &q)
        })
        .take(400)
        .collect::<Vec<_>>();

    // Stable selection: prefer session_id match, else keep previous index.
    let selected_idx_in_filtered = ctx
        .view
        .sessions
        .selected_session_id
        .as_deref()
        .and_then(|sid| {
            filtered
                .iter()
                .position(|row| row.session_id.as_deref() == Some(sid))
        })
        .unwrap_or(
            ctx.view
                .sessions
                .selected_idx
                .min(filtered.len().saturating_sub(1)),
        );

    ctx.view.sessions.selected_idx = selected_idx_in_filtered;
    let selected = filtered.get(ctx.view.sessions.selected_idx).copied();
    ctx.view.sessions.selected_session_id = selected.and_then(|r| r.session_id.clone());

    // Sync editor to the selected session, but do not clobber while editing the same session.
    if ctx.view.sessions.editor.sid != ctx.view.sessions.selected_session_id {
        ctx.view.sessions.editor.sid = ctx.view.sessions.selected_session_id.clone();
        ctx.view.sessions.editor.profile_selection = selected
            .and_then(|row| row.binding_profile_name.clone())
            .filter(|name| profiles.iter().any(|profile| profile.name == *name))
            .or_else(|| default_profile.clone())
            .or_else(|| profiles.first().map(|profile| profile.name.clone()));
        ctx.view.sessions.editor.model_override = selected
            .and_then(|r| r.override_model.clone())
            .unwrap_or_default();
        ctx.view.sessions.editor.config_override =
            selected.and_then(|r| r.override_station_name().map(str::to_owned));
        ctx.view.sessions.editor.effort_override = selected.and_then(|r| r.override_effort.clone());
        ctx.view.sessions.editor.custom_effort = selected
            .and_then(|r| r.override_effort.clone())
            .unwrap_or_default();
        ctx.view.sessions.editor.service_tier_override =
            selected.and_then(|r| r.override_service_tier.clone());
        ctx.view.sessions.editor.custom_service_tier = selected
            .and_then(|r| r.override_service_tier.clone())
            .unwrap_or_default();
    }

    let mut action_apply_session_profile: Option<(String, String)> = None;
    let mut action_clear_session_profile_binding: Option<String> = None;
    let mut action_clear_session_manual_overrides: Option<String> = None;

    ui.columns(2, |cols| {
        cols[0].heading(pick(ctx.lang, "列表", "List"));
        cols[0].add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("sessions_list_scroll")
            .max_height(520.0)
            .show(&mut cols[0], |ui| {
                for (pos, row) in filtered.iter().enumerate() {
                    let selected = pos == ctx.view.sessions.selected_idx;
                    let response = render_session_list_entry(ui, row, selected, ctx.lang);
                    if response.clicked() {
                        ctx.view.sessions.selected_idx = pos;
                        ctx.view.sessions.selected_session_id = row.session_id.clone();
                    }
                    ui.add_space(4.0);
                }
            });

        cols[1].heading(pick(ctx.lang, "详情", "Details"));
        cols[1].add_space(4.0);

        let Some(row) = selected else {
            cols[1].label(pick(ctx.lang, "无会话数据。", "No session data."));
            return;
        };
        cols[1].columns(2, |summary_cols| {
            render_session_identity_card(
                &mut summary_cols[0],
                ctx.lang,
                row,
                &profiles,
                host_local_session_features,
            );
            render_route_snapshot_card(
                &mut summary_cols[1],
                ctx.lang,
                row,
                global_station_override.as_deref(),
            );
        });
        cols[1].add_space(8.0);
        render_source_explanation_card(&mut cols[1], ctx.lang, row, has_session_cards);
        cols[1].add_space(8.0);

        console_section(
            &mut cols[1],
            pick(ctx.lang, "快捷操作", "Quick actions"),
            ConsoleTone::Neutral,
            |ui| {
                ui.horizontal_wrapped(|ui| {
            let can_copy = row.session_id.is_some();
            if ui
                .add_enabled(
                    can_copy,
                    egui::Button::new(pick(ctx.lang, "复制 session_id", "Copy session_id")),
                )
                .clicked()
                && let Some(sid) = row.session_id.as_deref()
            {
                ui.ctx().copy_text(sid.to_string());
                *ctx.last_info = Some(pick(ctx.lang, "已复制", "Copied").to_string());
            }

            let can_open_cwd = row.cwd.is_some() && host_local_session_features;
            let mut open_cwd = ui.add_enabled(
                can_open_cwd,
                egui::Button::new(pick(ctx.lang, "打开 cwd", "Open cwd")),
            );
            if row.cwd.is_none() {
                open_cwd = open_cwd.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前会话没有可用 cwd。",
                    "The current session has no cwd.",
                ));
            } else if !host_local_session_features {
                open_cwd = open_cwd.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前附着的是远端代理；这个 cwd 来自 host-local 观测，不一定存在于这台设备上。",
                    "A remote proxy is attached; this cwd came from host-local observation and may not exist on this device.",
                ));
            }

            if open_cwd.clicked()
                && let Some(cwd) = row.cwd.as_deref()
            {
                let path = std::path::PathBuf::from(cwd);
                if let Err(e) = open_in_file_manager(&path, false) {
                    *ctx.last_error = Some(format!("open cwd failed: {e}"));
                }
            }

            let can_open_requests = row.session_id.is_some();
            let mut open_requests = ui.add_enabled(
                can_open_requests,
                egui::Button::new(pick(ctx.lang, "在 Requests 查看", "Open in Requests")),
            );
            if row.session_id.is_none() {
                open_requests = open_requests.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前会话没有 session_id。",
                    "The current session has no session_id.",
                ));
            }
            if open_requests.clicked() {
                let Some(sid) = row.session_id.clone() else {
                    return;
                };
                prepare_select_requests_for_session(&mut ctx.view.requests, sid);
                ctx.view.requested_page = Some(Page::Requests);
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已切到 Requests 并限定到当前 session",
                        "Opened in Requests and scoped to the current session",
                    )
                    .to_string(),
                );
            }

            let can_open_history = row.session_id.is_some();
            let mut open_history = ui.add_enabled(
                can_open_history,
                egui::Button::new(pick(ctx.lang, "在 History 查看", "Open in History")),
            );
            if row.session_id.is_none() {
                open_history = open_history.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前会话没有 session_id。",
                    "The current session has no session_id.",
                ));
            }
            if open_history.clicked() {
                let Some(sid) = row.session_id.clone() else {
                    return;
                };
                let resolved_path = if host_local_session_features {
                    Ok(host_transcript_path_from_session_row(row))
                } else {
                    ctx.rt
                        .block_on(crate::sessions::find_codex_session_file_by_id(&sid))
                };
                match resolved_path {
                    Ok(path) => {
                        if let Some(summary) =
                            session_history_summary_from_row(row, path.clone(), ctx.lang)
                        {
                            history::prepare_select_session_from_external(
                                &mut ctx.view.history,
                                summary,
                                history::ExternalHistoryOrigin::Sessions,
                            );
                            ctx.view.requested_page = Some(Page::History);
                            *ctx.last_info = Some(
                                if path.is_some() {
                                    pick(
                                        ctx.lang,
                                        "已切到 History（本地 transcript）",
                                        "Opened in History (local transcript)",
                                    )
                                } else {
                                    pick(
                                        ctx.lang,
                                        "已切到 History（共享观测摘要）",
                                        "Opened in History (observed summary)",
                                    )
                                }
                                .to_string(),
                            );
                        }
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("find session file failed: {e}"));
                    }
                }
            }

            let can_open_transcript = row.session_id.is_some() && host_local_session_features;
            let mut open_transcript = ui.add_enabled(
                can_open_transcript,
                egui::Button::new(pick(ctx.lang, "打开对话记录", "Open transcript")),
            );
            if row.session_id.is_none() {
                open_transcript = open_transcript.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前会话没有 session_id。",
                    "The current session has no session_id.",
                ));
            } else if !host_local_session_features {
                open_transcript = open_transcript.on_disabled_hover_text(pick(
                    ctx.lang,
                    "当前附着的是远端代理；GUI 无法假设这台设备能直接读取远端 host 的 ~/.codex/sessions。",
                    "A remote proxy is attached; the GUI cannot assume this device can directly read the remote host's ~/.codex/sessions.",
                ));
            }
            if open_transcript.clicked()
            {
                let Some(sid) = row.session_id.clone() else {
                    return;
                };
                let resolved_path = if let Some(path) = host_transcript_path_from_session_row(row) {
                    Ok(Some(path))
                } else {
                    ctx.rt
                        .block_on(crate::sessions::find_codex_session_file_by_id(&sid))
                };
                match resolved_path {
                    Ok(Some(path)) => {
                        if let Some(summary) =
                            session_history_summary_from_row(row, Some(path), ctx.lang)
                        {
                            history::prepare_select_session_from_external(
                                &mut ctx.view.history,
                                summary,
                                history::ExternalHistoryOrigin::Sessions,
                            );
                            ctx.view.requested_page = Some(Page::History);
                        }
                    }
                    Ok(None) => {
                        *ctx.last_error = Some(pick(
                            ctx.lang,
                            "未找到该 session_id 的本地 Codex 会话文件（~/.codex/sessions）。",
                            "No local Codex session file found for this session_id (~/.codex/sessions).",
                        ).to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("find session file failed: {e}"));
                    }
                }
            }
                });
            },
        );
        if !host_local_session_features {
            if let Some(att) = ctx.proxy.attached()
                && let Some(warning) = remote_local_only_warning_message(
                    att.admin_base_url.as_str(),
                    &att.host_local_capabilities,
                    ctx.lang,
                    &[pick(ctx.lang, "cwd", "cwd"), pick(ctx.lang, "transcript", "transcript")],
                )
            {
                cols[1].small(warning);
            } else {
                cols[1].small(pick(
                    ctx.lang,
                    "提示：远端附着时，cwd / transcript 入口会被禁用；请用 Sessions / Requests 查看共享观测数据。",
                    "Tip: in remote-attached mode, cwd/transcript entries are disabled; use Sessions / Requests for shared observed data.",
                ));
            }
        }

        if let Some(status) = row.last_status {
            cols[1].label(format!("status(last): {status}"));
        }
        if let Some(ms) = row.last_duration_ms {
            cols[1].label(format!("duration(last): {ms} ms"));
        }
        if let Some(u) = row.last_usage.as_ref() {
            cols[1].label(format!("usage(last): {}", usage_line(u)));
        }
        if let Some(u) = row.total_usage.as_ref() {
            cols[1].label(format!("usage(total): {}", usage_line(u)));
        }

        cols[1].separator();

        let override_model = row.override_model.as_deref().unwrap_or("-");
        let override_cfg = row.override_station_name().unwrap_or("-");
        let override_eff = row.override_effort.as_deref().unwrap_or("-");
        let override_service_tier = row.override_service_tier.as_deref().unwrap_or("-");
        let global_cfg = global_station_override.as_deref().unwrap_or("-");
        cols[1].label(format!(
            "{}: model={override_model}, effort={override_eff}, station={override_cfg}, tier={override_service_tier}, global_station={global_cfg}",
            pick(ctx.lang, "覆盖", "Overrides")
        ));

        let Some(sid) = row.session_id.clone() else {
            cols[1].label(pick(
                ctx.lang,
                "该条目没有 session_id，暂不支持编辑覆盖。",
                "This entry has no session_id; overrides editing is disabled.",
            ));
            return;
        };

        let cfg_options = station_options_from_gui_stations(&snapshot.stations);
        let has_session_manual_overrides = row.override_model.is_some()
            || row.override_station_name().is_some()
            || row.override_effort.is_some()
            || row.override_service_tier.is_some();

        cols[1].add_space(6.0);
        cols[1].separator();
        render_last_route_decision_card(&mut cols[1], ctx.lang, row);

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "会话覆盖设置", "Session overrides"));
            let reset_overrides = ui
                .add_enabled(
                    snapshot.supports_session_override_reset && has_session_manual_overrides,
                    egui::Button::new(pick(
                        ctx.lang,
                        "重置 manual overrides",
                        "Reset manual overrides",
                    )),
                )
                .on_hover_text(pick(
                    ctx.lang,
                    "清除当前会话的 model / station / effort / service_tier 覆盖，不影响已绑定的 profile。",
                    "Clear the current session model / station / effort / service_tier overrides without touching the bound profile.",
                ));
            if reset_overrides.clicked() {
                action_clear_session_manual_overrides = Some(sid.clone());
            }
        });

        if profiles.is_empty() {
            cols[1].label(pick(
                ctx.lang,
                "当前未加载 control profile；可在 config.toml 的 [codex.profiles.*] 中定义。",
                "No control profiles loaded; define them in config.toml [codex.profiles.*].",
            ));
        } else {
            cols[1].horizontal_wrapped(|ui| {
                ui.label(pick(ctx.lang, "快捷应用", "Quick apply"));
                for profile in profiles.iter() {
                    let mut label =
                        format_profile_display(profile.name.as_str(), Some(profile));
                    if row.binding_profile_name.as_deref() == Some(profile.name.as_str()) {
                        label.push_str(match ctx.lang {
                            Language::Zh => " [当前绑定]",
                            Language::En => " [bound]",
                        });
                    }
                    let response =
                        ui.button(label).on_hover_text(format_profile_summary(profile));
                    if response.clicked() {
                        ctx.view.sessions.editor.profile_selection = Some(profile.name.clone());
                        action_apply_session_profile =
                            Some((sid.clone(), profile.name.clone()));
                    }
                }
            });

            cols[1].horizontal(|ui| {
                ui.label(pick(ctx.lang, "Profile binding", "Profile binding"));

                let mut selected_profile = ctx.view.sessions.editor.profile_selection.clone();
                egui::ComboBox::from_id_salt(("session_profile_apply", sid.as_str()))
                    .selected_text(match selected_profile.as_deref() {
                        Some(name) => format_profile_display(
                            name,
                            profiles.iter().find(|profile| profile.name == name),
                        ),
                        None => pick(ctx.lang, "<选择>", "<select>").to_string(),
                    })
                    .show_ui(ui, |ui| {
                        for profile in profiles.iter() {
                            ui.selectable_value(
                                &mut selected_profile,
                                Some(profile.name.clone()),
                                format_profile_display(profile.name.as_str(), Some(profile)),
                            );
                        }
                    });
                if selected_profile != ctx.view.sessions.editor.profile_selection {
                    ctx.view.sessions.editor.profile_selection = selected_profile;
                }

                if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                    if let Some(profile_name) = ctx.view.sessions.editor.profile_selection.clone() {
                        action_apply_session_profile = Some((sid.clone(), profile_name));
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

                let clear_binding = ui
                    .add_enabled(
                        row.binding_profile_name.is_some(),
                        egui::Button::new(pick(ctx.lang, "清除 binding", "Clear binding")),
                    )
                    .on_hover_text(pick(
                        ctx.lang,
                        "只移除当前会话已存储的 profile binding；保留 model / station / effort / service_tier 覆盖。",
                        "Only removes the stored session profile binding; keep model / station / effort / service_tier overrides.",
                    ));
                if clear_binding.clicked() {
                    action_clear_session_profile_binding = Some(sid.clone());
                }
            });

            if let Some(profile_name) = ctx.view.sessions.editor.profile_selection.as_deref()
                && let Some(profile) = profiles.iter().find(|profile| profile.name == profile_name)
            {
                cols[1].label(format!(
                    "{}: {}",
                    pick(ctx.lang, "Profile 详情", "Profile details"),
                    format_profile_summary(profile)
                ));
                let preview_profile =
                    match resolve_service_profile_from_options(profile_name, &profiles) {
                        Ok(profile) => profile,
                        Err(_) => service_profile_from_option(profile),
                    };
                let preview = build_profile_route_preview(
                    &preview_profile,
                    configured_active_station.as_deref(),
                    effective_active_station.as_deref(),
                    session_preview_station_specs,
                    session_preview_provider_catalog,
                    session_preview_runtime_station_catalog,
                );
                render_session_profile_apply_preview(
                    &mut cols[1],
                    ctx.lang,
                    row,
                    profile_name,
                    &preview_profile,
                    &preview,
                );
            }
        }

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "模型覆盖", "Model override"));
            ui.add(
                egui::TextEdit::singleline(&mut ctx.view.sessions.editor.model_override)
                    .desired_width(180.0)
                    .hint_text(pick(ctx.lang, "留空表示自动", "empty = auto")),
            );

            if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                let sid = sid.clone();
                let desired = {
                    let v = ctx.view.sessions.editor.model_override.trim().to_string();
                    if v.is_empty() { None } else { Some(v) }
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
                    match ctx.proxy.apply_session_model_override(ctx.rt, sid, desired) {
                        Ok(()) => {
                            force_refresh = true;
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

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "固定站点", "Pinned station"));

            let mut selected_name = ctx.view.sessions.editor.config_override.clone();
            egui::ComboBox::from_id_salt(("session_cfg_override", sid.as_str()))
                .selected_text(match selected_name.as_deref() {
                    Some(v) => v.to_string(),
                    None => pick(ctx.lang, "<自动>", "<auto>").to_string(),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected_name,
                        None,
                        pick(ctx.lang, "<自动>", "<auto>"),
                    );
                    for (name, label) in cfg_options.iter() {
                        ui.selectable_value(&mut selected_name, Some(name.clone()), label);
                    }
                });
            if selected_name != ctx.view.sessions.editor.config_override {
                ctx.view.sessions.editor.config_override = selected_name;
            }

            if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                let sid = sid.clone();
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
                        .apply_session_station_override(ctx.rt, sid, desired)
                    {
                        Ok(()) => {
                            force_refresh = true;
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

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "推理强度", "Reasoning effort"));

            let mut choice = match ctx.view.sessions.editor.effort_override.as_deref() {
                None => "auto",
                Some("low") => "low",
                Some("medium") => "medium",
                Some("high") => "high",
                Some("xhigh") => "xhigh",
                Some(_) => "custom",
            };

            egui::ComboBox::from_id_salt(("session_effort_choice", sid.as_str()))
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
                let sid = sid.clone();
                let desired = match choice {
                    "auto" => None,
                    "custom" => {
                        let v = ctx.view.sessions.editor.custom_effort.trim().to_string();
                        if v.is_empty() { None } else { Some(v) }
                    }
                    v => Some(v.to_string()),
                };
                match ctx
                    .proxy
                    .apply_session_effort_override(ctx.rt, sid, desired)
                {
                    Ok(()) => {
                        force_refresh = true;
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已应用覆盖", "Override applied").to_string());
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("apply override failed: {e}"));
                    }
                }
            }
        });

        cols[1].horizontal(|ui| {
            ui.label(pick(ctx.lang, "Fast / Service Tier", "Fast / Service tier"));

            let mut choice = match ctx.view.sessions.editor.service_tier_override.as_deref() {
                None => "auto",
                Some("default") => "default",
                Some("priority") => "priority",
                Some("flex") => "flex",
                Some(_) => "custom",
            };

            egui::ComboBox::from_id_salt(("session_service_tier_choice", sid.as_str()))
                .selected_text(match choice {
                    "priority" => pick(ctx.lang, "priority（fast）", "priority (fast)"),
                    v => v,
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
                let sid = sid.clone();
                let desired = match choice {
                    "auto" => None,
                    "custom" => {
                        let v = ctx
                            .view
                            .sessions
                            .editor
                            .custom_service_tier
                            .trim()
                            .to_string();
                        if v.is_empty() { None } else { Some(v) }
                    }
                    v => Some(v.to_string()),
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
                        .apply_session_service_tier_override(ctx.rt, sid, desired)
                    {
                        Ok(()) => {
                            force_refresh = true;
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
    });

    if let Some((sid, profile_name)) = action_apply_session_profile {
        match ctx
            .proxy
            .apply_session_profile(ctx.rt, sid, profile_name.clone())
        {
            Ok(()) => {
                force_refresh = true;
                *ctx.last_info = Some(format!(
                    "{}: {profile_name}",
                    pick(ctx.lang, "已应用 profile", "Profile applied")
                ));
            }
            Err(e) => {
                *ctx.last_error = Some(format!("apply profile failed: {e}"));
            }
        }
    }

    if let Some(sid) = action_clear_session_manual_overrides {
        match ctx.proxy.clear_session_manual_overrides(ctx.rt, sid) {
            Ok(()) => {
                force_refresh = true;
                ctx.view.sessions.editor.model_override.clear();
                ctx.view.sessions.editor.config_override = None;
                ctx.view.sessions.editor.effort_override = None;
                ctx.view.sessions.editor.custom_effort.clear();
                ctx.view.sessions.editor.service_tier_override = None;
                ctx.view.sessions.editor.custom_service_tier.clear();
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已重置 session manual overrides",
                        "Session manual overrides reset",
                    )
                    .to_string(),
                );
            }
            Err(e) => {
                *ctx.last_error = Some(format!("reset session manual overrides failed: {e}"));
            }
        }
    }
    if let Some(sid) = action_clear_session_profile_binding {
        match ctx.proxy.clear_session_profile_binding(ctx.rt, sid) {
            Ok(()) => {
                force_refresh = true;
                ctx.view.sessions.editor.profile_selection = default_profile
                    .clone()
                    .or_else(|| profiles.first().map(|profile| profile.name.clone()));
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已清除 profile binding",
                        "Profile binding cleared",
                    )
                    .to_string(),
                );
            }
            Err(e) => {
                *ctx.last_error = Some(format!("clear profile binding failed: {e}"));
            }
        }
    }
    if force_refresh {
        ctx.proxy
            .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
    }
}

fn render_route_snapshot_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    global_station_override: Option<&str>,
) {
    let observed_rows = vec![
        (
            "model(last)".to_string(),
            row.last_model.as_deref().unwrap_or("-").to_string(),
        ),
        (
            "station(last)".to_string(),
            row.last_station_name().unwrap_or("-").to_string(),
        ),
        (
            "upstream(last)".to_string(),
            row.last_upstream_base_url
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "effort(last)".to_string(),
            row.last_reasoning_effort
                .as_deref()
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "service_tier(last)".to_string(),
            row.last_service_tier.as_deref().unwrap_or("-").to_string(),
        ),
    ];
    let effective_rows = vec![
        (
            "model".to_string(),
            format_resolved_route_value(row.effective_model.as_ref(), lang),
        ),
        (
            "station".to_string(),
            format_resolved_route_value(row.effective_station(), lang),
        ),
        (
            "upstream".to_string(),
            format_resolved_route_value(row.effective_upstream_base_url.as_ref(), lang),
        ),
        (
            "effort".to_string(),
            format_resolved_route_value(row.effective_reasoning_effort.as_ref(), lang),
        ),
        (
            "service_tier".to_string(),
            format_resolved_route_value(row.effective_service_tier.as_ref(), lang),
        ),
    ];
    let override_summary = format!(
        "model={}, effort={}, station={}, tier={}, global_station={}",
        row.override_model.as_deref().unwrap_or("-"),
        row.override_effort.as_deref().unwrap_or("-"),
        row.override_station_name().unwrap_or("-"),
        row.override_service_tier.as_deref().unwrap_or("-"),
        global_station_override.unwrap_or("-"),
    );

    console_section(
        ui,
        pick(lang, "路由快照", "Route snapshot"),
        ConsoleTone::Accent,
        |ui| {
            ui.columns(2, |cols| {
                cols[0].label(pick(lang, "最近观测", "Observed"));
                console_kv_grid(
                    &mut cols[0],
                    (
                        "sessions_route_observed_grid",
                        row.session_id.as_deref().unwrap_or("<aggregate>"),
                    ),
                    &observed_rows,
                );

                cols[1].label(pick(lang, "当前生效", "Effective"));
                console_kv_grid(
                    &mut cols[1],
                    (
                        "sessions_route_effective_grid",
                        row.session_id.as_deref().unwrap_or("<aggregate>"),
                    ),
                    &effective_rows,
                );
            });
            ui.add_space(6.0);
            console_note(
                ui,
                format!(
                    "{}: {override_summary}",
                    pick(lang, "覆盖概览", "Override summary")
                ),
            );
        },
    );
}

fn render_source_explanation_card(
    ui: &mut egui::Ui,
    lang: Language,
    row: &SessionRow,
    has_session_cards: bool,
) {
    console_section(
        ui,
        pick(lang, "来源解释", "Source explanation"),
        ConsoleTone::Neutral,
        |ui| {
            egui::Grid::new((
                "sessions_effective_route_explanation_grid",
                row.session_id.as_deref().unwrap_or("<aggregate>"),
            ))
            .num_columns(3)
            .spacing([12.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong(pick(lang, "字段", "Field"));
                ui.strong(pick(lang, "当前值 / 来源", "Value / source"));
                ui.strong(pick(lang, "为什么", "Why"));
                ui.end_row();

                for field in EffectiveRouteField::ALL {
                    let explanation = explain_effective_route_field(row, field, lang);
                    ui.label(effective_route_field_label(field, lang));
                    ui.vertical(|ui| {
                        ui.monospace(explanation.value);
                        ui.small(format!("[{}]", explanation.source_label));
                    });
                    ui.small(explanation.reason);
                    ui.end_row();
                }
            });
            if !has_session_cards {
                ui.add_space(6.0);
                console_note(
                    ui,
                    pick(
                        lang,
                        "当前附着数据来自旧接口回退，这里的来源解释是 best effort 推导。",
                        "Current attach data came from legacy fallback endpoints, so this explanation is best effort.",
                    ),
                );
            }
        },
    );
}
