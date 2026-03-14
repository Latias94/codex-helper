use super::components::console_layout::{ConsoleTone, console_note, console_section};
use super::*;

pub(super) fn render_session_quick_actions(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
    host_local_session_features: bool,
) {
    console_section(
        ui,
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
                if open_transcript.clicked() {
                    let Some(sid) = row.session_id.clone() else {
                        return;
                    };
                    let resolved_path = if let Some(path) = host_transcript_path_from_session_row(row)
                    {
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
                            *ctx.last_error = Some(
                                pick(
                                    ctx.lang,
                                    "未找到该 session_id 的本地 Codex 会话文件（~/.codex/sessions）。",
                                    "No local Codex session file found for this session_id (~/.codex/sessions).",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => {
                            *ctx.last_error = Some(format!("find session file failed: {e}"));
                        }
                    }
                }
            });
        },
    );
}

pub(super) fn render_source_explanation_card(
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
            super::render_effective_route_explanation_grid(
                ui,
                lang,
                row,
                "sessions_effective_route_explanation_grid",
            );
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
