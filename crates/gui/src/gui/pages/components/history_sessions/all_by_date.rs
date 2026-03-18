use super::*;

pub(crate) fn render_all_by_date_dates_panel(
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

pub(crate) fn render_all_by_date_sessions_panel(
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
        let row_h = 42.0;
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
                    let branch = ctx
                        .view
                        .history
                        .branch_by_workdir
                        .get(root_or_cwd.as_str())
                        .and_then(|v| v.as_deref())
                        .unwrap_or("-");
                    let root_short = shorten_middle(root_or_cwd.as_str(), 64);
                    let t_short = shorten(t, 19);
                    let first_short = shorten(first, 58);
                    let line1 = if branch == "-" {
                        format!("{root_short}  {t_short}")
                    } else {
                        format!("{root_short}  [{branch}]  {t_short}")
                    };
                    let label = format!("{line1}\n{}  {first_short}", s.id);
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
