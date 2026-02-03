use eframe::egui;

use super::super::super::i18n::{Language, pick};
use super::super::history::HistoryScope;
use super::super::{
    PageCtx, WtItemSkipReason, basename, build_wt_items_from_session_summaries, format_age,
    history_workdir_from_cwd, now_ms, open_wt_items, path_mtime_ms, short_sid, shorten,
    workdir_status_from_cwd,
};

pub(in super::super) fn render_sessions_panel_horizontal(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
) -> Option<(usize, String)> {
    ui.heading(pick(ctx.lang, "会话列表", "Sessions"));
    ui.add_space(4.0);

    let mut action_batch_select_visible = false;
    let mut action_batch_clear = false;
    let mut pending_select: Option<(usize, String)> = None;

    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "全选可见", "Select visible"))
            .clicked()
        {
            action_batch_select_visible = true;
        }
        if ui.button(pick(ctx.lang, "清空选择", "Clear")).clicked() {
            action_batch_clear = true;
        }
    });
    ui.add_space(4.0);

    let mut visible_ids: Vec<String> = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("history_sessions_scroll")
        .max_height(520.0)
        .show(ui, |ui| {
            let group_enabled =
                ctx.view.history.scope == HistoryScope::GlobalRecent && ctx.view.history.group_by_workdir;

            if group_enabled {
                #[derive(Debug)]
                struct WorkdirGroup {
                    key: String,
                    indices: Vec<usize>,
                    last_mtime_ms: u64,
                }

                let now = now_ms();
                let mut order: Vec<String> = Vec::new();
                let mut groups: std::collections::HashMap<String, WorkdirGroup> =
                    std::collections::HashMap::new();

                for (idx, s) in ctx.view.history.sessions.iter().enumerate() {
                    let key = s
                        .cwd
                        .as_deref()
                        .map(|cwd| history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root))
                        .unwrap_or_else(|| "-".to_string());
                    let mtime_ms = path_mtime_ms(s.path.as_path());

                    if !groups.contains_key(key.as_str()) {
                        order.push(key.clone());
                        groups.insert(
                            key.clone(),
                            WorkdirGroup {
                                key: key.clone(),
                                indices: Vec::new(),
                                last_mtime_ms: mtime_ms,
                            },
                        );
                    }
                    if let Some(g) = groups.get_mut(key.as_str()) {
                        g.indices.push(idx);
                        g.last_mtime_ms = g.last_mtime_ms.max(mtime_ms);
                    }
                }

                let mut ordered = order
                    .into_iter()
                    .filter_map(|k| groups.remove(k.as_str()))
                    .collect::<Vec<_>>();
                ordered.sort_by_key(|g| std::cmp::Reverse(g.last_mtime_ms));

                for g in ordered.into_iter() {
                    let key = g.key.clone();
                    let mut ok_indices: Vec<usize> = Vec::new();
                    let mut skipped_missing_cwd = 0usize;
                    let mut skipped_invalid_workdir = 0usize;
                    let mut skipped_missing_dir = 0usize;
                    for &idx in g.indices.iter() {
                        let reason = ctx
                            .view
                            .history
                            .sessions
                            .get(idx)
                            .and_then(|s| {
                                workdir_status_from_cwd(
                                    s.cwd.as_deref(),
                                    ctx.view.history.infer_git_root,
                                )
                                .err()
                            });
                        match reason {
                            None => ok_indices.push(idx),
                            Some(WtItemSkipReason::MissingCwd) => skipped_missing_cwd += 1,
                            Some(WtItemSkipReason::InvalidWorkdir) => skipped_invalid_workdir += 1,
                            Some(WtItemSkipReason::WorkdirNotFound) => skipped_missing_dir += 1,
                        }
                    }
                    let ok_n = ok_indices.len();
                    let collapsed = ctx.view.history.collapsed_workdirs.contains(&key);
                    let name = if key == "-" {
                        pick(ctx.lang, "<未知目录>", "<unknown>").to_string()
                    } else {
                        shorten(basename(key.as_str()), 34)
                    };
                    let age = if g.last_mtime_ms > 0 {
                        format_age(now, Some(g.last_mtime_ms))
                    } else {
                        "-".to_string()
                    };
                    let n = g.indices.len();

                    ui.horizontal(|ui| {
                        if ui.small_button(if collapsed { "▸" } else { "▾" }).clicked() {
                            if collapsed {
                                ctx.view.history.collapsed_workdirs.remove(&key);
                            } else {
                                ctx.view.history.collapsed_workdirs.insert(key.clone());
                            }
                        }

                        let header = format!("{name}  ok={ok_n}/{n}  {age}");
                        let mut hover = String::new();
                        hover.push_str(key.as_str());
                        if skipped_missing_cwd + skipped_invalid_workdir + skipped_missing_dir > 0 {
                            hover.push('\n');
                            hover.push_str(&format!(
                                "skipped: missing_cwd={skipped_missing_cwd}, invalid_workdir={skipped_invalid_workdir}, missing_dir={skipped_missing_dir}"
                            ));
                        }
                        ui.label(header).on_hover_text(hover);

                        if ui.small_button(pick(ctx.lang, "全选", "Select")).clicked() {
                            for &idx in g.indices.iter() {
                                if let Some(s) = ctx.view.history.sessions.get(idx) {
                                    ctx.view.history.batch_selected_ids.insert(s.id.clone());
                                }
                            }
                        }
                        if ui.small_button(pick(ctx.lang, "清空", "Clear")).clicked() {
                            for &idx in g.indices.iter() {
                                if let Some(s) = ctx.view.history.sessions.get(idx) {
                                    ctx.view.history.batch_selected_ids.remove(&s.id);
                                }
                            }
                        }

                        let can_open = cfg!(windows) && ok_n > 0;
                        let label = match ctx.lang {
                            Language::Zh => format!("打开({ok_n})"),
                            Language::En => format!("Open ({ok_n})"),
                        };
                        if ui.add_enabled(can_open, egui::Button::new(label)).clicked() {
                            let items = build_wt_items_from_session_summaries(
                                ok_indices.iter().filter_map(|&idx| ctx.view.history.sessions.get(idx)),
                                ctx.view.history.infer_git_root,
                                ctx.view.history.resume_cmd.as_str(),
                            );
                            open_wt_items(ctx, items);
                        }

                        let mut open_n = ctx.view.history.group_open_recent_n.max(1);
                        ui.label("N");
                        ui.add(egui::DragValue::new(&mut open_n).range(1..=50).speed(1));
                        if open_n != ctx.view.history.group_open_recent_n {
                            ctx.view.history.group_open_recent_n = open_n;
                            ctx.gui_cfg.history.group_open_recent_n = open_n;
                            if let Err(e) = ctx.gui_cfg.save() {
                                *ctx.last_error = Some(format!("save gui config failed: {e}"));
                            }
                        }

                        let open_n = open_n.min(ok_n);
                        let label_n = match ctx.lang {
                            Language::Zh => format!("打开最近{open_n}"),
                            Language::En => format!("Open top {open_n}"),
                        };
                        if ui
                            .add_enabled(can_open, egui::Button::new(label_n))
                            .clicked()
                        {
                            let items = build_wt_items_from_session_summaries(
                                ok_indices
                                    .iter()
                                    .take(open_n)
                                    .filter_map(|&idx| ctx.view.history.sessions.get(idx)),
                                ctx.view.history.infer_git_root,
                                ctx.view.history.resume_cmd.as_str(),
                            );
                            open_wt_items(ctx, items);
                        }
                    });

                    if collapsed {
                        ui.add_space(4.0);
                        continue;
                    }

                    for &idx in g.indices.iter() {
                        let Some(s) = ctx.view.history.sessions.get(idx) else {
                            continue;
                        };
                        visible_ids.push(s.id.clone());

                        let selected = idx == ctx.view.history.selected_idx;
                        let id_short = short_sid(&s.id, 16);
                        let rounds = s.rounds;
                        let last = s
                            .last_response_at
                            .as_deref()
                            .or(s.updated_at.as_deref())
                            .unwrap_or("-");
                        let first = s.first_user_message.as_deref().unwrap_or("-");
                        let label = format!(
                            "{id_short}  r={rounds}  {last}  {}",
                            shorten(first, 46)
                        );

                        let sid = s.id.clone();
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            let mut checked = ctx.view.history.batch_selected_ids.contains(&sid);
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    ctx.view.history.batch_selected_ids.insert(sid.clone());
                                } else {
                                    ctx.view.history.batch_selected_ids.remove(&sid);
                                }
                            }

                            if ui.selectable_label(selected, label).clicked() {
                                pending_select = Some((idx, sid.clone()));
                            }
                        });
                    }
                    ui.add_space(6.0);
                }
            } else {
                for (idx, s) in ctx.view.history.sessions.iter().enumerate() {
                    visible_ids.push(s.id.clone());

                    let selected = idx == ctx.view.history.selected_idx;
                    let id_short = short_sid(&s.id, 16);
                    let rounds = s.rounds;
                    let last = s
                        .last_response_at
                        .as_deref()
                        .or(s.updated_at.as_deref())
                        .unwrap_or("-");
                    let first = s.first_user_message.as_deref().unwrap_or("-");
                    let label = match ctx.view.history.scope {
                        HistoryScope::CurrentProject => {
                            let cwd = s
                                .cwd
                                .as_deref()
                                .map(|v| shorten(basename(v), 22))
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "{id_short}  r={rounds}  {cwd}  {last}  {}",
                                shorten(first, 40)
                            )
                        }
                        HistoryScope::GlobalRecent | HistoryScope::AllByDate => {
                            let root = s
                                .cwd
                                .as_deref()
                                .map(|cwd| {
                                    history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root)
                                })
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "{}  {id_short}  r={rounds}  {last}  {}",
                                shorten(&root, 44),
                                shorten(first, 36)
                            )
                        }
                    };
                    let sid = s.id.clone();
                    ui.horizontal(|ui| {
                        let mut checked = ctx.view.history.batch_selected_ids.contains(&sid);
                        if ui.checkbox(&mut checked, "").changed() {
                            if checked {
                                ctx.view.history.batch_selected_ids.insert(sid.clone());
                            } else {
                                ctx.view.history.batch_selected_ids.remove(&sid);
                            }
                        }

                        if ui.selectable_label(selected, label).clicked() {
                            pending_select = Some((idx, sid.clone()));
                        }
                    });
                }
            }
        });

    if action_batch_clear {
        ctx.view.history.batch_selected_ids.clear();
    }
    if action_batch_select_visible {
        for sid in visible_ids.iter() {
            ctx.view.history.batch_selected_ids.insert(sid.clone());
        }
    }

    pending_select
}

pub(in super::super) fn render_sessions_panel_vertical(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
) -> Option<(usize, String)> {
    ui.heading(pick(ctx.lang, "会话列表", "Sessions"));
    ui.add_space(4.0);

    let mut action_batch_select_visible = false;
    let mut action_batch_clear = false;
    let mut pending_select: Option<(usize, String)> = None;
    let mut visible_ids: Vec<String> = Vec::new();

    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "全选可见", "Select visible"))
            .clicked()
        {
            action_batch_select_visible = true;
        }
        if ui.button(pick(ctx.lang, "清空选择", "Clear")).clicked() {
            action_batch_clear = true;
        }
    });
    ui.add_space(4.0);

    let list_max_h = ui.available_height().max(200.0);
    egui::ScrollArea::vertical()
        .id_salt("history_sessions_scroll")
        .max_height(list_max_h)
        .show(ui, |ui| {
            let group_enabled = ctx.view.history.scope == HistoryScope::GlobalRecent
                && ctx.view.history.group_by_workdir;

            if group_enabled {
                #[derive(Debug)]
                struct WorkdirGroup {
                    key: String,
                    indices: Vec<usize>,
                    last_mtime_ms: u64,
                }

                let now = now_ms();
                let mut order: Vec<String> = Vec::new();
                let mut groups: std::collections::HashMap<String, WorkdirGroup> =
                    std::collections::HashMap::new();

                for (idx, s) in ctx.view.history.sessions.iter().enumerate() {
                    let key = s
                        .cwd
                        .as_deref()
                        .map(|cwd| history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root))
                        .unwrap_or_else(|| "-".to_string());
                    let mtime_ms = path_mtime_ms(s.path.as_path());

                    if !groups.contains_key(key.as_str()) {
                        order.push(key.clone());
                        groups.insert(
                            key.clone(),
                            WorkdirGroup {
                                key: key.clone(),
                                indices: Vec::new(),
                                last_mtime_ms: mtime_ms,
                            },
                        );
                    }
                    if let Some(g) = groups.get_mut(key.as_str()) {
                        g.indices.push(idx);
                        g.last_mtime_ms = g.last_mtime_ms.max(mtime_ms);
                    }
                }

                let mut ordered = order
                    .into_iter()
                    .filter_map(|k| groups.remove(k.as_str()))
                    .collect::<Vec<_>>();
                ordered.sort_by_key(|g| std::cmp::Reverse(g.last_mtime_ms));

                for g in ordered.into_iter() {
                    let key = g.key.clone();
                    let ok_n = g.indices.len();

                    let header = match ctx.lang {
                        Language::Zh => format!("{}  ({})", shorten(&key, 44), ok_n),
                        Language::En => format!("{}  ({})", shorten(&key, 44), ok_n),
                    };

                    let mut collapsed = ctx.view.history.collapsed_workdirs.contains(key.as_str());

                    ui.horizontal(|ui| {
                        if ui.selectable_label(!collapsed, header.as_str()).clicked() {
                            collapsed = !collapsed;
                            if collapsed {
                                ctx.view.history.collapsed_workdirs.insert(key.clone());
                            } else {
                                ctx.view.history.collapsed_workdirs.remove(key.as_str());
                            }
                        }

                        ui.add_space(6.0);
                        ui.colored_label(
                            egui::Color32::from_gray(120),
                            format!(
                                "{}{}",
                                pick(ctx.lang, "更新", "Updated"),
                                format_age(now, Some(g.last_mtime_ms))
                            ),
                        );
                    });

                    if collapsed {
                        ui.add_space(4.0);
                        continue;
                    }

                    for &idx in g.indices.iter() {
                        let Some(s) = ctx.view.history.sessions.get(idx) else {
                            continue;
                        };
                        visible_ids.push(s.id.clone());

                        let selected = idx == ctx.view.history.selected_idx;
                        let id_short = short_sid(&s.id, 16);
                        let rounds = s.rounds;
                        let last = s
                            .last_response_at
                            .as_deref()
                            .or(s.updated_at.as_deref())
                            .unwrap_or("-");
                        let first = s.first_user_message.as_deref().unwrap_or("-");
                        let label =
                            format!("{id_short}  r={rounds}  {last}  {}", shorten(first, 46));

                        let sid = s.id.clone();
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            let mut checked = ctx.view.history.batch_selected_ids.contains(&sid);
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    ctx.view.history.batch_selected_ids.insert(sid.clone());
                                } else {
                                    ctx.view.history.batch_selected_ids.remove(&sid);
                                }
                            }

                            if ui.selectable_label(selected, label).clicked() {
                                pending_select = Some((idx, sid.clone()));
                            }
                        });
                    }
                    ui.add_space(6.0);
                }
            } else {
                for (idx, s) in ctx.view.history.sessions.iter().enumerate() {
                    visible_ids.push(s.id.clone());

                    let selected = idx == ctx.view.history.selected_idx;
                    let id_short = short_sid(&s.id, 16);
                    let rounds = s.rounds;
                    let last = s
                        .last_response_at
                        .as_deref()
                        .or(s.updated_at.as_deref())
                        .unwrap_or("-");
                    let first = s.first_user_message.as_deref().unwrap_or("-");
                    let label = match ctx.view.history.scope {
                        HistoryScope::CurrentProject => {
                            let cwd = s
                                .cwd
                                .as_deref()
                                .map(|v| shorten(basename(v), 22))
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "{id_short}  r={rounds}  {cwd}  {last}  {}",
                                shorten(first, 40)
                            )
                        }
                        HistoryScope::GlobalRecent | HistoryScope::AllByDate => {
                            let root = s
                                .cwd
                                .as_deref()
                                .map(|cwd| {
                                    history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root)
                                })
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "{}  {id_short}  r={rounds}  {last}  {}",
                                shorten(&root, 44),
                                shorten(first, 36)
                            )
                        }
                    };

                    let sid = s.id.clone();
                    ui.horizontal(|ui| {
                        let mut checked = ctx.view.history.batch_selected_ids.contains(&sid);
                        if ui.checkbox(&mut checked, "").changed() {
                            if checked {
                                ctx.view.history.batch_selected_ids.insert(sid.clone());
                            } else {
                                ctx.view.history.batch_selected_ids.remove(&sid);
                            }
                        }

                        if ui.selectable_label(selected, label).clicked() {
                            pending_select = Some((idx, sid.clone()));
                        }
                    });
                }
            }
        });

    if action_batch_clear {
        ctx.view.history.batch_selected_ids.clear();
    }
    if action_batch_select_visible {
        for sid in visible_ids.iter() {
            ctx.view.history.batch_selected_ids.insert(sid.clone());
        }
    }

    pending_select
}

pub(in super::super) fn render_all_by_date_dates_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    max_height: f32,
    scroll_id_salt: &'static str,
) {
    ui.heading(pick(ctx.lang, "日期", "Dates"));
    ui.add_space(4.0);

    let total = ctx.view.history.all_dates.len();
    let row_h = 22.0;
    egui::ScrollArea::vertical()
        .id_salt(scroll_id_salt)
        .max_height(max_height)
        .show_rows(ui, row_h, total, |ui, range| {
            for row in range {
                let d = &ctx.view.history.all_dates[row];
                let selected = ctx
                    .view
                    .history
                    .all_selected_date
                    .as_deref()
                    .is_some_and(|x| x == d.date);
                if ui.selectable_label(selected, d.date.as_str()).clicked() {
                    ctx.view.history.all_selected_date = Some(d.date.clone());
                    ctx.view.history.loaded_day_for = None;
                }
            }
        });
}

pub(in super::super) fn render_all_by_date_sessions_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    query: &str,
    max_height: f32,
    scroll_id_salt: &'static str,
) -> Option<(usize, String)> {
    ui.heading(pick(ctx.lang, "会话", "Sessions"));
    ui.add_space(4.0);

    let mut action_batch_select_visible = false;
    let mut action_batch_clear = false;
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "全选可见", "Select visible"))
            .clicked()
        {
            action_batch_select_visible = true;
        }
        if ui.button(pick(ctx.lang, "清空选择", "Clear")).clicked() {
            action_batch_clear = true;
        }
    });
    ui.add_space(4.0);

    let q = query.trim();
    let mut visible_indices: Vec<usize> = Vec::new();
    for (idx, s) in ctx.view.history.all_day_sessions.iter().enumerate() {
        if q.is_empty() {
            visible_indices.push(idx);
            continue;
        }
        let mut matched = false;
        if let Some(cwd) = s.cwd.as_deref() {
            matched |= cwd.to_lowercase().contains(q);
        }
        if let Some(msg) = s.first_user_message.as_deref() {
            matched |= msg.to_lowercase().contains(q);
        }
        if matched {
            visible_indices.push(idx);
        }
    }

    let mut pending_select: Option<(usize, String)> = None;
    {
        let total = visible_indices.len();
        let row_h = 22.0;
        egui::ScrollArea::vertical()
            .id_salt(scroll_id_salt)
            .max_height(max_height)
            .show_rows(ui, row_h, total, |ui, range| {
                for row in range {
                    let idx = visible_indices[row];
                    let s = &ctx.view.history.all_day_sessions[idx];
                    let selected = ctx
                        .view
                        .history
                        .selected_id
                        .as_deref()
                        .is_some_and(|id| id == s.id);

                    let id_short = short_sid(&s.id, 16);
                    let t = s
                        .updated_hint
                        .as_deref()
                        .or(s.created_at.as_deref())
                        .unwrap_or("-");
                    let root_or_cwd = s
                        .cwd
                        .as_deref()
                        .map(|cwd| {
                            if ctx.view.history.infer_git_root {
                                crate::sessions::infer_project_root_from_cwd(cwd)
                            } else {
                                cwd.to_string()
                            }
                        })
                        .unwrap_or_else(|| "-".to_string());
                    let first = s.first_user_message.as_deref().unwrap_or("-");
                    let label = format!(
                        "{}  {}  {}  {}",
                        shorten(&root_or_cwd, 36),
                        id_short,
                        shorten(t, 19),
                        shorten(first, 40)
                    );
                    let sid = s.id.clone();
                    ui.horizontal(|ui| {
                        let mut checked = ctx.view.history.batch_selected_ids.contains(&sid);
                        if ui.checkbox(&mut checked, "").changed() {
                            if checked {
                                ctx.view.history.batch_selected_ids.insert(sid.clone());
                            } else {
                                ctx.view.history.batch_selected_ids.remove(&sid);
                            }
                        }

                        if ui.selectable_label(selected, label).clicked() {
                            pending_select = Some((idx, sid.clone()));
                        }
                    });
                }
            });
    }

    if action_batch_clear {
        ctx.view.history.batch_selected_ids.clear();
    }
    if action_batch_select_visible {
        for &idx in visible_indices.iter() {
            if let Some(s) = ctx.view.history.all_day_sessions.get(idx) {
                ctx.view.history.batch_selected_ids.insert(s.id.clone());
            }
        }
    }

    pending_select
}
