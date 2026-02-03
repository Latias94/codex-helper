use eframe::egui;

use super::super::super::i18n::{Language, pick};
use super::super::super::util::{open_in_file_manager, spawn_windows_terminal_wt_new_tab};
use super::super::history::cancel_transcript_load;
use super::super::{
    PageCtx, build_wt_items_from_session_summaries, open_wt_items, workdir_status_from_cwd,
};

use crate::sessions::{SessionIndexItem, SessionSummary};

pub(in super::super) enum BatchOpenSource<'a> {
    Summaries(&'a [SessionSummary]),
    DaySessions(&'a [SessionIndexItem]),
}

fn resume_cmd_for_id(template: &str, selected_id: &str) -> String {
    let t = template.trim();
    if t.is_empty() {
        format!("codex resume {selected_id}")
    } else if t.contains("{id}") {
        t.replace("{id}", selected_id)
    } else {
        format!("{t} {selected_id}")
    }
}

fn build_wt_items_from_day_sessions<'a, I>(
    sessions: I,
    infer_git_root: bool,
    resume_cmd_template: &str,
) -> Vec<(String, String)>
where
    I: IntoIterator<Item = &'a SessionIndexItem>,
{
    let mut out = Vec::new();
    let t = resume_cmd_template.trim();
    for s in sessions.into_iter() {
        let Ok(workdir) = workdir_status_from_cwd(s.cwd.as_deref(), infer_git_root) else {
            continue;
        };

        let sid = s.id.as_str();
        let cmd = if t.is_empty() {
            format!("codex resume {sid}")
        } else if t.contains("{id}") {
            t.replace("{id}", sid)
        } else {
            format!("{t} {sid}")
        };
        out.push((workdir, cmd));
    }
    out
}

pub(in super::super) fn render_resume_group(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    source: BatchOpenSource<'_>,
    batch_mode_id_salt: &'static str,
) {
    ui.label(pick(ctx.lang, "恢复", "Resume"));

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "命令模板", "Template"));
        let resp = ui
            .add(egui::TextEdit::singleline(&mut ctx.view.history.resume_cmd).desired_width(260.0));
        if resp.lost_focus() && ctx.gui_cfg.history.resume_cmd != ctx.view.history.resume_cmd {
            ctx.gui_cfg.history.resume_cmd = ctx.view.history.resume_cmd.clone();
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
        if ui
            .button(pick(ctx.lang, "用 bypass", "Use bypass"))
            .clicked()
        {
            ctx.view.history.resume_cmd =
                "codex --dangerously-bypass-approvals-and-sandbox resume {id}".to_string();
            ctx.gui_cfg.history.resume_cmd = ctx.view.history.resume_cmd.clone();
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "Shell", "Shell"));
        let resp =
            ui.add(egui::TextEdit::singleline(&mut ctx.view.history.shell).desired_width(140.0));
        if resp.lost_focus() && ctx.gui_cfg.history.shell != ctx.view.history.shell {
            ctx.gui_cfg.history.shell = ctx.view.history.shell.clone();
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
        let mut keep_open = ctx.view.history.keep_open;
        ui.checkbox(&mut keep_open, pick(ctx.lang, "保持打开", "Keep open"));
        if keep_open != ctx.view.history.keep_open {
            ctx.view.history.keep_open = keep_open;
            ctx.gui_cfg.history.keep_open = keep_open;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "批量打开", "Batch open"));

        let mut mode = ctx
            .gui_cfg
            .history
            .wt_batch_mode
            .trim()
            .to_ascii_lowercase();
        if mode != "tabs" && mode != "windows" {
            mode = "tabs".to_string();
        }
        let mut selected_mode = mode.clone();
        egui::ComboBox::from_id_salt(batch_mode_id_salt)
            .selected_text(match selected_mode.as_str() {
                "windows" => pick(ctx.lang, "每会话新窗口", "Window per session"),
                _ => pick(ctx.lang, "单窗口多标签", "One window (tabs)"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut selected_mode,
                    "tabs".to_string(),
                    pick(ctx.lang, "单窗口多标签", "One window (tabs)"),
                );
                ui.selectable_value(
                    &mut selected_mode,
                    "windows".to_string(),
                    pick(ctx.lang, "每会话新窗口", "Window per session"),
                );
            });
        if selected_mode != mode {
            ctx.gui_cfg.history.wt_batch_mode = selected_mode;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }

        render_open_selected_in_wt_button(ui, ctx, source);
    });
}

pub(in super::super) fn render_open_selected_in_wt_button(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    source: BatchOpenSource<'_>,
) {
    let n = ctx.view.history.batch_selected_ids.len();
    let label = match ctx.lang {
        Language::Zh => format!("在 wt 中打开选中({n})"),
        Language::En => format!("Open selected in wt ({n})"),
    };
    let can_open = cfg!(windows) && n > 0;
    if !ui.add_enabled(can_open, egui::Button::new(label)).clicked() {
        return;
    }

    let items = match source {
        BatchOpenSource::Summaries(list) => build_wt_items_from_session_summaries(
            list.iter()
                .filter(|s| ctx.view.history.batch_selected_ids.contains(&s.id)),
            ctx.view.history.infer_git_root,
            ctx.view.history.resume_cmd.as_str(),
        ),
        BatchOpenSource::DaySessions(list) => build_wt_items_from_day_sessions(
            list.iter()
                .filter(|s| ctx.view.history.batch_selected_ids.contains(&s.id)),
            ctx.view.history.infer_git_root,
            ctx.view.history.resume_cmd.as_str(),
        ),
    };

    open_wt_items(ctx, items);
}

pub(in super::super) fn render_selected_session_actions(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    selected_id: &str,
    workdir: &str,
    selected_path: &std::path::Path,
) {
    let resume_cmd = resume_cmd_for_id(ctx.view.history.resume_cmd.as_str(), selected_id);

    if ui
        .button(pick(ctx.lang, "复制 root+id", "Copy root+id"))
        .clicked()
    {
        if workdir.trim().is_empty() || workdir == "-" {
            *ctx.last_error = Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
        } else {
            ui.ctx()
                .copy_text(format!("{} {}", workdir.trim(), selected_id));
            *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
        }
    }

    if ui
        .button(pick(ctx.lang, "复制 resume", "Copy resume"))
        .clicked()
    {
        ui.ctx().copy_text(resume_cmd.clone());
        *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
    }

    if cfg!(windows)
        && ui
            .button(pick(ctx.lang, "在 wt 中恢复", "Open in wt"))
            .clicked()
    {
        if workdir.trim().is_empty() || workdir == "-" {
            *ctx.last_error = Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
        } else if !std::path::Path::new(workdir.trim()).exists() {
            *ctx.last_error = Some(format!(
                "{}: {}",
                pick(ctx.lang, "目录不存在", "Directory not found"),
                workdir.trim()
            ));
        } else if let Err(e) = spawn_windows_terminal_wt_new_tab(
            ctx.view.history.wt_window,
            workdir.trim(),
            ctx.view.history.shell.trim(),
            ctx.view.history.keep_open,
            &resume_cmd,
        ) {
            *ctx.last_error = Some(format!("spawn wt failed: {e}"));
        }
    }

    if ui.button(pick(ctx.lang, "打开文件", "Open file")).clicked()
        && let Err(e) = open_in_file_manager(selected_path, true)
    {
        *ctx.last_error = Some(format!("open session failed: {e}"));
    }
}

pub(in super::super) fn reset_transcript_view_after_session_switch(ctx: &mut PageCtx<'_>) {
    ctx.view.history.loaded_for = None;
    cancel_transcript_load(&mut ctx.view.history);
    ctx.view.history.transcript_raw_messages.clear();
    ctx.view.history.transcript_messages.clear();
    ctx.view.history.transcript_error = None;
    ctx.view.history.transcript_plain_key = None;
    ctx.view.history.transcript_plain_text.clear();
    ctx.view.history.transcript_selected_msg_idx = 0;
    ctx.view.history.transcript_scroll_to_msg_idx = None;
}
