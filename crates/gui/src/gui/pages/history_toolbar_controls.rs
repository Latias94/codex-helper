use eframe::egui;

use super::history::{HistoryDataSource, HistoryScope};
use super::history_controller::{apply_tail_transcript_search, refresh_history_sessions};
use super::history_toolbar_all_by_date::render_all_by_date_controls;
use super::history_toolbar_recent::render_global_recent_controls;
use super::*;

fn copy_root_id_list(ctx: &mut PageCtx<'_>, ui: &egui::Ui) {
    let mut out = String::new();
    for session in ctx.view.history.sessions.iter() {
        let cwd = session.cwd.as_deref().unwrap_or("-");
        if cwd == "-" {
            continue;
        }
        let root = if ctx.view.history.infer_git_root {
            crate::sessions::infer_project_root_from_cwd(cwd)
        } else {
            cwd.to_string()
        };
        out.push_str(root.trim());
        out.push(' ');
        out.push_str(session.id.as_str());
        out.push('\n');
    }
    ui.ctx().copy_text(out);
    *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
}

fn render_history_scope_controls(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    refresh_requested: &mut bool,
) {
    ui.label(pick(ctx.lang, "范围", "Scope"));
    egui::ComboBox::from_id_salt("history_scope")
        .selected_text(match ctx.view.history.scope {
            HistoryScope::CurrentProject => pick(ctx.lang, "当前项目", "Current project"),
            HistoryScope::GlobalRecent => pick(ctx.lang, "全局最近", "Global recent"),
            HistoryScope::AllByDate => pick(ctx.lang, "全部(按日期)", "All (by date)"),
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut ctx.view.history.scope,
                HistoryScope::CurrentProject,
                pick(ctx.lang, "当前项目", "Current project"),
            );
            ui.selectable_value(
                &mut ctx.view.history.scope,
                HistoryScope::GlobalRecent,
                pick(ctx.lang, "全局最近", "Global recent"),
            );
            ui.selectable_value(
                &mut ctx.view.history.scope,
                HistoryScope::AllByDate,
                pick(ctx.lang, "全部(按日期)", "All (by date)"),
            );
        });

    if ctx.view.history.scope == HistoryScope::GlobalRecent {
        render_global_recent_controls(ui, ctx, refresh_requested);
    } else if ctx.view.history.scope == HistoryScope::AllByDate {
        render_all_by_date_controls(ui, ctx);
    }
}

fn render_history_layout_controls(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.separator();
    ui.label(pick(ctx.lang, "布局", "Layout"));
    let mut mode = ctx.view.history.layout_mode.trim().to_ascii_lowercase();
    if mode != "auto" && mode != "horizontal" && mode != "vertical" {
        mode = "auto".to_string();
    }
    let mut selected_mode = mode.clone();
    egui::ComboBox::from_id_salt("history_layout_mode")
        .selected_text(match selected_mode.as_str() {
            "horizontal" => pick(ctx.lang, "左右", "Horizontal"),
            "vertical" => pick(ctx.lang, "上下", "Vertical"),
            _ => pick(ctx.lang, "自动", "Auto"),
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut selected_mode,
                "auto".to_string(),
                pick(ctx.lang, "自动", "Auto"),
            );
            ui.selectable_value(
                &mut selected_mode,
                "horizontal".to_string(),
                pick(ctx.lang, "左右", "Horizontal"),
            );
            ui.selectable_value(
                &mut selected_mode,
                "vertical".to_string(),
                pick(ctx.lang, "上下", "Vertical"),
            );
        });
    if selected_mode != mode {
        ctx.view.history.layout_mode = selected_mode.clone();
        ctx.gui_cfg.history.layout_mode = selected_mode;
        if let Err(error) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {error}"));
        }
    }
}

pub(super) fn render_history_controls(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    shared_observed_history_available: bool,
    refresh_requested: &mut bool,
) {
    ui.horizontal(|ui| {
        render_history_scope_controls(ui, ctx, refresh_requested);
        render_history_layout_controls(ui, ctx);
    });

    if ctx.view.history.scope == HistoryScope::AllByDate {
        return;
    }

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(240.0)
                .hint_text(pick(
                    ctx.lang,
                    if ctx.view.history.scope == HistoryScope::GlobalRecent {
                        "输入关键词（匹配 cwd 或首条用户消息）"
                    } else {
                        "输入关键词（匹配首条用户消息）"
                    },
                    if ctx.view.history.scope == HistoryScope::GlobalRecent {
                        "keyword (cwd or first user message)"
                    } else {
                        "keyword (first user message)"
                    },
                )),
        );

        let mut action_apply_tail_search = false;
        let observed_fallback_active =
            ctx.view.history.data_source == HistoryDataSource::ObservedFallback;
        ui.add_enabled_ui(!observed_fallback_active, |ui| {
            ui.checkbox(
                &mut ctx.view.history.search_transcript_tail,
                pick(ctx.lang, "搜对话(尾部)", "Transcript (tail)"),
            )
            .on_hover_text(pick(
                ctx.lang,
                "可选：在元信息不命中时，再扫描每个会话文件尾部的 N 条消息（更慢，但更像 cc-switch 的全文搜索）。",
                "Optional: if metadata doesn't match, scan the last N messages (slower, closer to cc-switch full-text).",
            ));
        });
        if observed_fallback_active {
            ui.small(pick(
                ctx.lang,
                "共享观测模式下没有本地 transcript 文件，因此这里只做元信息过滤。",
                "Observed mode has no local transcript files, so only metadata filtering is available here.",
            ));
        }
        if ctx.view.history.search_transcript_tail && !observed_fallback_active {
            ui.label(pick(ctx.lang, "N", "N"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.search_transcript_tail_n)
                    .range(10..=500)
                    .speed(1),
            );
            if ui.button(pick(ctx.lang, "应用", "Apply")).clicked() {
                action_apply_tail_search = true;
            }
        }

        if *refresh_requested || ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            refresh_history_sessions(ctx, shared_observed_history_available);
        }

        if ctx.view.history.scope == HistoryScope::GlobalRecent
            && ui
                .button(pick(ctx.lang, "复制 root+id 列表", "Copy root+id list"))
                .clicked()
        {
            copy_root_id_list(ctx, ui);
        }

        ui.add_enabled_ui(
            ctx.view.history.data_source == HistoryDataSource::LocalFiles,
            |ui| {
                ui.checkbox(
                    &mut ctx.view.history.auto_load_transcript,
                    pick(ctx.lang, "自动加载对话", "Auto load transcript"),
                );
            },
        );

        if action_apply_tail_search {
            apply_tail_transcript_search(ctx);
        }
    });
}
