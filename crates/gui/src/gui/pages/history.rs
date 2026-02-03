use eframe::egui;
use std::collections::HashSet;

use super::super::i18n::{Language, pick};
use super::super::util::{
    open_in_file_manager, spawn_windows_terminal_wt_new_tab,
    spawn_windows_terminal_wt_tabs_in_one_window,
};
use super::PageCtx;
use super::components::{history_sessions, history_transcript};
use super::{
    history_workdir_from_cwd, now_ms, open_wt_items, short_sid, shorten,
    sort_session_summaries_by_mtime_desc,
};

use crate::sessions::{SessionDayDir, SessionIndexItem, SessionSummary, SessionTranscriptMessage};

#[derive(Debug)]
pub struct HistoryViewState {
    pub scope: HistoryScope,
    pub query: String,
    pub sessions_all: Vec<SessionSummary>,
    pub sessions: Vec<SessionSummary>,
    pub last_error: Option<String>,
    pub loaded_at_ms: Option<u64>,
    pub selected_idx: usize,
    pub selected_id: Option<String>,
    applied_scope: HistoryScope,
    applied_query: String,
    pub recent_since_hours: u32,
    pub recent_limit: usize,
    pub infer_git_root: bool,
    pub resume_cmd: String,
    pub shell: String,
    pub keep_open: bool,
    pub layout_mode: String,
    pub sessions_panel_height: f32,
    pub wt_window: i32,
    pub batch_selected_ids: HashSet<String>,
    pub group_by_workdir: bool,
    pub collapsed_workdirs: HashSet<String>,
    pub group_open_recent_n: usize,
    pub all_days_limit: usize,
    pub all_dates: Vec<SessionDayDir>,
    pub all_selected_date: Option<String>,
    pub all_day_limit: usize,
    pub all_day_sessions: Vec<SessionIndexItem>,
    loaded_day_for: Option<String>,
    pub search_transcript_tail: bool,
    pub search_transcript_tail_n: usize,
    search_transcript_applied: Option<(HistoryScope, String, usize)>,
    pub hide_tool_calls: bool,
    pub transcript_view: TranscriptViewMode,
    pub transcript_selected_msg_idx: usize,
    pub transcript_find_query: String,
    pub transcript_find_case_sensitive: bool,
    pub(super) transcript_scroll_to_msg_idx: Option<usize>,
    pub(super) transcript_plain_key: Option<(String, Option<usize>, bool)>,
    pub(super) transcript_plain_text: String,
    transcript_load_seq: u64,
    pub(super) transcript_load: Option<TranscriptLoad>,
    pub auto_load_transcript: bool,
    pub transcript_full: bool,
    pub transcript_tail: usize,
    pub transcript_raw_messages: Vec<SessionTranscriptMessage>,
    pub transcript_messages: Vec<SessionTranscriptMessage>,
    pub transcript_error: Option<String>,
    pub(super) loaded_for: Option<(String, Option<usize>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryScope {
    CurrentProject,
    GlobalRecent,
    AllByDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedHistoryLayout {
    Horizontal,
    Vertical,
}

fn resolve_history_layout(layout_mode: &str, available_width: f32) -> ResolvedHistoryLayout {
    match layout_mode.trim().to_ascii_lowercase().as_str() {
        "horizontal" | "h" => ResolvedHistoryLayout::Horizontal,
        "vertical" | "v" => ResolvedHistoryLayout::Vertical,
        "auto" | "" => {
            if available_width < 980.0 {
                ResolvedHistoryLayout::Vertical
            } else {
                ResolvedHistoryLayout::Horizontal
            }
        }
        _ => {
            if available_width < 980.0 {
                ResolvedHistoryLayout::Vertical
            } else {
                ResolvedHistoryLayout::Horizontal
            }
        }
    }
}

impl Default for HistoryViewState {
    fn default() -> Self {
        Self {
            scope: HistoryScope::CurrentProject,
            query: String::new(),
            sessions_all: Vec::new(),
            sessions: Vec::new(),
            last_error: None,
            loaded_at_ms: None,
            selected_idx: 0,
            selected_id: None,
            applied_scope: HistoryScope::CurrentProject,
            applied_query: String::new(),
            recent_since_hours: 12,
            recent_limit: 50,
            infer_git_root: false,
            resume_cmd: "codex resume {id}".to_string(),
            shell: "pwsh".to_string(),
            keep_open: true,
            layout_mode: "auto".to_string(),
            sessions_panel_height: 280.0,
            wt_window: -1,
            batch_selected_ids: HashSet::new(),
            group_by_workdir: true,
            collapsed_workdirs: HashSet::new(),
            group_open_recent_n: 5,
            all_days_limit: 120,
            all_dates: Vec::new(),
            all_selected_date: None,
            all_day_limit: 500,
            all_day_sessions: Vec::new(),
            loaded_day_for: None,
            search_transcript_tail: false,
            search_transcript_tail_n: 80,
            search_transcript_applied: None,
            hide_tool_calls: true,
            transcript_view: TranscriptViewMode::Messages,
            transcript_selected_msg_idx: 0,
            transcript_find_query: String::new(),
            transcript_find_case_sensitive: false,
            transcript_scroll_to_msg_idx: None,
            transcript_plain_key: None,
            transcript_plain_text: String::new(),
            transcript_load_seq: 0,
            transcript_load: None,
            auto_load_transcript: true,
            transcript_full: false,
            transcript_tail: 80,
            transcript_raw_messages: Vec::new(),
            transcript_messages: Vec::new(),
            transcript_error: None,
            loaded_for: None,
        }
    }
}

pub(super) fn prepare_select_session_from_external(
    state: &mut HistoryViewState,
    selected_idx: usize,
    sid: String,
) {
    state.selected_idx = selected_idx;
    state.selected_id = Some(sid);
    state.loaded_for = None;
    state.auto_load_transcript = true;
    cancel_transcript_load(state);
    state.transcript_raw_messages.clear();
    state.transcript_messages.clear();
    state.transcript_error = None;
    state.transcript_scroll_to_msg_idx = None;
    state.transcript_plain_key = None;
    state.transcript_plain_text.clear();
    state.transcript_selected_msg_idx = 0;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptViewMode {
    Messages,
    PlainText,
}

#[derive(Debug)]
pub(in crate::gui::pages) struct TranscriptLoad {
    seq: u64,
    key: (String, Option<usize>),
    rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<Vec<SessionTranscriptMessage>>)>,
    join: tokio::task::JoinHandle<()>,
}

pub(super) fn render_history(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    poll_transcript_loader(ctx);

    ui.heading(pick(ctx.lang, "历史会话", "History"));
    ui.label(pick(
        ctx.lang,
        "读取 Codex 的本地 sessions（~/.codex/sessions）。",
        "Reads local Codex sessions (~/.codex/sessions).",
    ));

    ui.add_space(6.0);

    ui.horizontal(|ui| {
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
            ui.label(pick(ctx.lang, "最近(小时)", "Since (hours)"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.recent_since_hours)
                    .range(1..=168)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "条数", "Limit"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.recent_limit)
                    .range(1..=500)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "工作目录", "Workdir"));
            let mut mode = ctx.gui_cfg.history.workdir_mode.trim().to_ascii_lowercase();
            if mode != "cwd" && mode != "git_root" {
                mode = "cwd".to_string();
            }
            let mut selected_mode = mode.clone();
            egui::ComboBox::from_id_salt("history_workdir_mode")
                .selected_text(match selected_mode.as_str() {
                    "git_root" => pick(ctx.lang, "git 根目录", "git root"),
                    _ => pick(ctx.lang, "会话 cwd", "session cwd"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected_mode,
                        "cwd".to_string(),
                        pick(ctx.lang, "会话 cwd", "session cwd"),
                    )
                    .on_hover_text(pick(
                        ctx.lang,
                        "使用会话记录中的 cwd 作为恢复/复制的工作目录（推荐）",
                        "Use the session's cwd as workdir (recommended).",
                    ));
                    ui.selectable_value(
                        &mut selected_mode,
                        "git_root".to_string(),
                        pick(ctx.lang, "git 根目录", "git root"),
                    )
                    .on_hover_text(pick(
                        ctx.lang,
                        "在 cwd 上向上查找 .git 作为项目根目录（用于复制/打开）",
                        "Find .git upward from cwd as project root (for copy/open).",
                    ));
                });
            if selected_mode != mode {
                ctx.gui_cfg.history.workdir_mode = selected_mode.clone();
                ctx.view.history.infer_git_root = selected_mode == "git_root";
                if let Err(e) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {e}"));
                }
            }

            ui.checkbox(
                &mut ctx.view.history.group_by_workdir,
                pick(ctx.lang, "按项目分组", "Group by project"),
            )
            .on_hover_text(pick(
                ctx.lang,
                "按工作目录分组并折叠，适合“第二天继续昨天的一堆会话”的批量恢复。",
                "Group by workdir with collapsible headers; great for batch resume next day.",
            ));
        } else if ctx.view.history.scope == HistoryScope::AllByDate {
            ui.label(pick(ctx.lang, "最近天数", "Recent days"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.all_days_limit)
                    .range(1..=10_000)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "当日上限", "Day limit"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.all_day_limit)
                    .range(1..=10_000)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "工作目录", "Workdir"));
            let mut mode = ctx.gui_cfg.history.workdir_mode.trim().to_ascii_lowercase();
            if mode != "cwd" && mode != "git_root" {
                mode = "cwd".to_string();
            }
            let mut selected_mode = mode.clone();
            egui::ComboBox::from_id_salt("history_workdir_mode_all_by_date")
                .selected_text(match selected_mode.as_str() {
                    "git_root" => pick(ctx.lang, "git 根目录", "git root"),
                    _ => pick(ctx.lang, "会话 cwd", "session cwd"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected_mode,
                        "cwd".to_string(),
                        pick(ctx.lang, "会话 cwd", "session cwd"),
                    );
                    ui.selectable_value(
                        &mut selected_mode,
                        "git_root".to_string(),
                        pick(ctx.lang, "git 根目录", "git root"),
                    );
                });
            if selected_mode != mode {
                ctx.gui_cfg.history.workdir_mode = selected_mode.clone();
                ctx.view.history.infer_git_root = selected_mode == "git_root";
                if let Err(e) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {e}"));
                }
            }
        }

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
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    if ctx.view.history.scope == HistoryScope::AllByDate {
        render_history_all_by_date(ui, ctx);
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
        ui.checkbox(
            &mut ctx.view.history.search_transcript_tail,
            pick(ctx.lang, "搜对话(尾部)", "Transcript (tail)"),
        )
        .on_hover_text(pick(
            ctx.lang,
            "可选：在元信息不命中时，再扫描每个会话文件尾部的 N 条消息（更慢，但更像 cc-switch 的全文搜索）。",
            "Optional: if metadata doesn't match, scan the last N messages (slower, closer to cc-switch full-text).",
        ));
        if ctx.view.history.search_transcript_tail {
            ui.label(pick(ctx.lang, "N", "N"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.search_transcript_tail_n)
                    .range(10..=500)
                    .speed(1),
            );
            if ui
                .button(pick(ctx.lang, "应用", "Apply"))
                .clicked()
            {
                action_apply_tail_search = true;
            }
        }

        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            let scope = ctx.view.history.scope;
            let recent_since_hours = ctx.view.history.recent_since_hours;
            let recent_limit = ctx.view.history.recent_limit;
            let fut = async move {
                match scope {
                    HistoryScope::CurrentProject => {
                        crate::sessions::find_codex_sessions_for_current_dir(200).await
                    }
                    HistoryScope::GlobalRecent => {
                        let since = std::time::Duration::from_secs(
                            (recent_since_hours as u64).saturating_mul(3600),
                        );
                        crate::sessions::find_recent_codex_session_summaries(since, recent_limit)
                            .await
                    }
                    HistoryScope::AllByDate => Ok(Vec::new()),
                }
            };
            match ctx.rt.block_on(fut) {
                Ok(mut list) => {
                    sort_session_summaries_by_mtime_desc(&mut list);
                    ctx.view.history.sessions_all = list;
                    ctx.view.history.search_transcript_applied = None;
                    ctx.view.history.loaded_at_ms = Some(now_ms());
                    ctx.view.history.last_error = None;

                    // Re-apply current metadata filter without hitting disk again.
                    let q = ctx.view.history.query.trim().to_lowercase();
                    let scope = ctx.view.history.scope;
                    ctx.view.history.sessions = if q.is_empty() {
                        ctx.view.history.sessions_all.clone()
                    } else {
                        ctx.view
                            .history
                            .sessions_all
                            .iter()
                            .filter(|&s| match scope {
                                HistoryScope::GlobalRecent => s
                                    .cwd
                                    .as_deref()
                                    .is_some_and(|cwd| cwd.to_lowercase().contains(q.as_str()))
                                    || s.first_user_message.as_deref().is_some_and(|msg| {
                                        msg.to_lowercase().contains(q.as_str())
                                    }),
                                _ => s.first_user_message.as_deref().is_some_and(|msg| {
                                    msg.to_lowercase().contains(q.as_str())
                                }),
                            })
                            .cloned()
                            .collect()
                    };
                    ctx.view.history.applied_scope = scope;
                    ctx.view.history.applied_query = ctx.view.history.query.clone();

                    if ctx.view.history.sessions.is_empty() {
                        ctx.view.history.selected_idx = 0;
                        ctx.view.history.selected_id = None;
                        cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.loaded_for = None;
                    } else if ctx
                        .view
                        .history
                        .selected_id
                        .as_deref()
                        .is_none_or(|id| !ctx.view.history.sessions.iter().any(|s| s.id == id))
                    {
                        ctx.view.history.selected_idx = 0;
                        ctx.view.history.selected_id =
                            Some(ctx.view.history.sessions[0].id.clone());
                        ctx.view.history.loaded_for = None;
                        cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.transcript_plain_key = None;
                        ctx.view.history.transcript_plain_text.clear();
                    }
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        if ctx.view.history.scope == HistoryScope::GlobalRecent
            && ui
                .button(pick(ctx.lang, "复制 root+id 列表", "Copy root+id list"))
                .clicked()
        {
            let mut out = String::new();
            for s in ctx.view.history.sessions.iter() {
                let cwd = s.cwd.as_deref().unwrap_or("-");
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
                out.push_str(s.id.as_str());
                out.push('\n');
            }
            ui.ctx().copy_text(out);
            *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
        }

        ui.checkbox(
            &mut ctx.view.history.auto_load_transcript,
            pick(ctx.lang, "自动加载对话", "Auto load transcript"),
        );

        if action_apply_tail_search {
            let q = ctx.view.history.query.trim().to_string();
            if q.is_empty() {
                *ctx.last_error = Some(pick(
                    ctx.lang,
                    "请输入关键词后再应用“搜对话(尾部)”",
                    "Enter a query before applying transcript search",
                ).to_string());
            } else {
                let scope = ctx.view.history.scope;
                let tail = ctx.view.history.search_transcript_tail_n;
                let all = ctx.view.history.sessions_all.clone();
                let needle = q.clone();
                let mut out: Vec<SessionSummary> = Vec::new();
                let needle_lc = needle.to_lowercase();
                let meta_match = |s: &SessionSummary| -> bool {
                    match scope {
                        HistoryScope::GlobalRecent => s
                            .cwd
                            .as_deref()
                            .is_some_and(|cwd| cwd.to_lowercase().contains(needle_lc.as_str()))
                            || s.first_user_message.as_deref().is_some_and(|msg| {
                                msg.to_lowercase().contains(needle_lc.as_str())
                            }),
                        _ => s.first_user_message.as_deref().is_some_and(|msg| {
                            msg.to_lowercase().contains(needle_lc.as_str())
                        }),
                    }
                };

                let fut = async move {
                    for s in all.into_iter() {
                        if meta_match(&s) {
                            out.push(s);
                            continue;
                        }
                        if crate::sessions::codex_session_transcript_tail_contains_query(
                            &s.path,
                            &needle,
                            tail,
                        )
                        .await?
                        {
                            out.push(s);
                        }
                    }
                    Ok::<Vec<SessionSummary>, anyhow::Error>(out)
                };
                match ctx.rt.block_on(fut) {
                    Ok(list) => {
                        ctx.view.history.sessions = list;
                        ctx.view.history.search_transcript_applied = Some((scope, q, tail));
                        ctx.view.history.applied_scope = scope;
                        ctx.view.history.applied_query = ctx.view.history.query.clone();
                        ctx.view.history.selected_idx = 0;
                        ctx.view.history.selected_id =
                            ctx.view.history.sessions.first().map(|s| s.id.clone());
                        ctx.view.history.loaded_for = None;
                        cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.transcript_plain_key = None;
                        ctx.view.history.transcript_plain_text.clear();
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已应用全文过滤", "Applied").to_string());
                    }
                    Err(e) => {
                        ctx.view.history.last_error = Some(e.to_string());
                    }
                }
            }
        }
    });

    // Apply lightweight (metadata-only) filtering immediately when query/scope changes.
    if (ctx.view.history.applied_scope != ctx.view.history.scope
        || ctx.view.history.applied_query != ctx.view.history.query)
        && !matches!(ctx.view.history.scope, HistoryScope::AllByDate)
    {
        ctx.view.history.applied_scope = ctx.view.history.scope;
        ctx.view.history.applied_query = ctx.view.history.query.clone();
        ctx.view.history.search_transcript_applied = None;

        let q = ctx.view.history.query.trim().to_lowercase();
        if q.is_empty() {
            ctx.view.history.sessions = ctx.view.history.sessions_all.clone();
        } else {
            let scope = ctx.view.history.scope;
            ctx.view.history.sessions = ctx
                .view
                .history
                .sessions_all
                .iter()
                .filter(|s| match scope {
                    HistoryScope::GlobalRecent => {
                        s.cwd
                            .as_deref()
                            .is_some_and(|cwd| cwd.to_lowercase().contains(q.as_str()))
                            || s.first_user_message
                                .as_deref()
                                .is_some_and(|msg| msg.to_lowercase().contains(q.as_str()))
                    }
                    _ => s
                        .first_user_message
                        .as_deref()
                        .is_some_and(|msg| msg.to_lowercase().contains(q.as_str())),
                })
                .cloned()
                .collect();
        }

        // If selection falls out, reset and clear transcript.
        let selected_ok = ctx
            .view
            .history
            .selected_id
            .as_deref()
            .is_some_and(|id| ctx.view.history.sessions.iter().any(|s| s.id == id));
        if !selected_ok {
            ctx.view.history.selected_idx = 0;
            ctx.view.history.selected_id = ctx.view.history.sessions.first().map(|s| s.id.clone());
            ctx.view.history.loaded_for = None;
            cancel_transcript_load(&mut ctx.view.history);
            ctx.view.history.transcript_raw_messages.clear();
            ctx.view.history.transcript_messages.clear();
            ctx.view.history.transcript_error = None;
            ctx.view.history.transcript_plain_key = None;
            ctx.view.history.transcript_plain_text.clear();
        }
    }

    if let Some(err) = ctx.view.history.last_error.as_deref() {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }

    if ctx.view.history.sessions.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "暂无会话。点击“刷新”加载。",
            "No sessions loaded. Click Refresh.",
        ));
        return;
    }

    // Keep selection stable.
    let selected_idx = ctx
        .view
        .history
        .selected_id
        .as_deref()
        .and_then(|id| ctx.view.history.sessions.iter().position(|s| s.id == id))
        .unwrap_or(
            ctx.view
                .history
                .selected_idx
                .min(ctx.view.history.sessions.len().saturating_sub(1)),
        );
    ctx.view.history.selected_idx = selected_idx;
    ctx.view.history.selected_id = Some(ctx.view.history.sessions[selected_idx].id.clone());

    if ctx.view.history.auto_load_transcript
        && let Some(id) = ctx.view.history.selected_id.clone()
    {
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        let key = (id.clone(), tail);
        let path = ctx.view.history.sessions[selected_idx].path.clone();
        ensure_transcript_loading(ctx, path, key);
    }

    ui.add_space(6.0);
    let layout =
        resolve_history_layout(ctx.view.history.layout_mode.as_str(), ui.available_width());
    if layout == ResolvedHistoryLayout::Vertical {
        render_history_vertical(ui, ctx);
        return;
    }
    ui.columns(2, |cols| {
        let pending_select = history_sessions::render_sessions_panel_horizontal(&mut cols[0], ctx);

        if let Some((idx, id)) = pending_select {
            ctx.view.history.selected_idx = idx;
            ctx.view.history.selected_id = Some(id);
            ctx.view.history.loaded_for = None;
            cancel_transcript_load(&mut ctx.view.history);
            ctx.view.history.transcript_raw_messages.clear();
            ctx.view.history.transcript_messages.clear();
            ctx.view.history.transcript_error = None;
            ctx.view.history.transcript_plain_key = None;
            ctx.view.history.transcript_plain_text.clear();
            ctx.view.history.transcript_selected_msg_idx = 0;
        }

        cols[1].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[1].add_space(4.0);

        let selected_idx = ctx
            .view
            .history
            .selected_idx
            .min(ctx.view.history.sessions.len().saturating_sub(1));
        let selected = &ctx.view.history.sessions[selected_idx];
        let selected_id = selected.id.clone();
        let selected_cwd = selected.cwd.clone().unwrap_or_else(|| "-".to_string());
        let workdir =
            history_workdir_from_cwd(selected_cwd.as_str(), ctx.view.history.infer_git_root);
        let resume_cmd = {
            let t = ctx.view.history.resume_cmd.trim();
            if t.is_empty() {
                format!("codex resume {selected_id}")
            } else if t.contains("{id}") {
                t.replace("{id}", &selected_id)
            } else {
                format!("{t} {selected_id}")
            }
        };

        cols[1].group(|ui| {
            ui.label(pick(ctx.lang, "恢复", "Resume"));

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "命令模板", "Template"));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.resume_cmd)
                        .desired_width(260.0),
                );
                if resp.lost_focus()
                    && ctx.gui_cfg.history.resume_cmd != ctx.view.history.resume_cmd
                {
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
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.shell).desired_width(140.0),
                );
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
                egui::ComboBox::from_id_salt("history_wt_batch_mode")
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

                let n = ctx.view.history.batch_selected_ids.len();
                let label = match ctx.lang {
                    Language::Zh => format!("在 wt 中打开选中({n})"),
                    Language::En => format!("Open selected in wt ({n})"),
                };
                let can_open = cfg!(windows) && n > 0;
                if ui.add_enabled(can_open, egui::Button::new(label)).clicked() {
                    let items = ctx
                        .view
                        .history
                        .sessions
                        .iter()
                        .filter(|s| ctx.view.history.batch_selected_ids.contains(&s.id))
                        .filter_map(|s| {
                            let cwd = s.cwd.as_deref()?.trim().to_string();
                            if cwd.is_empty() || cwd == "-" {
                                return None;
                            }
                            let workdir = history_workdir_from_cwd(
                                cwd.as_str(),
                                ctx.view.history.infer_git_root,
                            );
                            if !std::path::Path::new(workdir.as_str()).exists() {
                                return None;
                            }
                            let sid = s.id.clone();
                            let cmd = {
                                let t = ctx.view.history.resume_cmd.trim();
                                if t.is_empty() {
                                    format!("codex resume {sid}")
                                } else if t.contains("{id}") {
                                    t.replace("{id}", &sid)
                                } else {
                                    format!("{t} {sid}")
                                }
                            };
                            Some((workdir, cmd))
                        })
                        .collect::<Vec<_>>();

                    if items.is_empty() {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "没有可打开的会话（cwd 不可用或目录不存在）",
                                "No sessions to open (cwd unavailable or missing)",
                            )
                            .to_string(),
                        );
                    } else {
                        let mode = ctx
                            .gui_cfg
                            .history
                            .wt_batch_mode
                            .trim()
                            .to_ascii_lowercase();
                        let shell = ctx.view.history.shell.trim();
                        let keep_open = ctx.view.history.keep_open;

                        let result = if mode == "windows" {
                            let mut last_err: Option<anyhow::Error> = None;
                            for (cwd, cmd) in items.iter() {
                                if let Err(e) = spawn_windows_terminal_wt_new_tab(
                                    -1,
                                    cwd.as_str(),
                                    shell,
                                    keep_open,
                                    cmd.as_str(),
                                ) {
                                    last_err = Some(e);
                                    break;
                                }
                            }
                            match last_err {
                                Some(e) => Err(e),
                                None => Ok(()),
                            }
                        } else {
                            spawn_windows_terminal_wt_tabs_in_one_window(&items, shell, keep_open)
                        };

                        match result {
                            Ok(()) => {
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已启动 Windows Terminal",
                                        "Started Windows Terminal",
                                    )
                                    .to_string(),
                                );
                            }
                            Err(e) => *ctx.last_error = Some(format!("spawn wt failed: {e}")),
                        }
                    }
                }
            });

            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "复制 root+id", "Copy root+id"))
                    .clicked()
                {
                    if workdir.trim().is_empty() || workdir == "-" {
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
                    } else {
                        ui.ctx()
                            .copy_text(format!("{} {}", workdir.trim(), selected_id));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
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
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
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
            });

            ui.label(format!("id: {}", selected_id));
            ui.label(format!("dir: {}", workdir));

            if let Some(first) = selected.first_user_message.as_deref() {
                let mut text = first.to_string();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            }
        });

        cols[1].horizontal(|ui| {
            let mut hide = ctx.view.history.hide_tool_calls;
            ui.checkbox(&mut hide, pick(ctx.lang, "隐藏工具调用", "Hide tool calls"));
            if hide != ctx.view.history.hide_tool_calls {
                ctx.view.history.hide_tool_calls = hide;
                ctx.view.history.transcript_messages = history_transcript::filter_tool_calls(
                    ctx.view.history.transcript_raw_messages.clone(),
                    hide,
                );
                ctx.view.history.transcript_plain_key = None;
                ctx.view.history.transcript_plain_text.clear();
            }

            ui.label(pick(ctx.lang, "显示", "View"));
            egui::ComboBox::from_id_salt("history_transcript_view")
                .selected_text(match ctx.view.history.transcript_view {
                    TranscriptViewMode::Messages => pick(ctx.lang, "消息列表", "Messages"),
                    TranscriptViewMode::PlainText => pick(ctx.lang, "纯文本", "Plain text"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::Messages,
                        pick(ctx.lang, "消息列表", "Messages"),
                    );
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::PlainText,
                        pick(ctx.lang, "纯文本", "Plain text"),
                    );
                });

            let mut full = ctx.view.history.transcript_full;
            ui.checkbox(&mut full, pick(ctx.lang, "全量", "All"));
            if full != ctx.view.history.transcript_full {
                ctx.view.history.transcript_full = full;
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
            }

            ui.label(pick(ctx.lang, "尾部条数", "Tail"));
            ui.add_enabled(
                !ctx.view.history.transcript_full,
                egui::DragValue::new(&mut ctx.view.history.transcript_tail)
                    .range(10..=500)
                    .speed(1),
            );
            if ctx.view.history.transcript_full {
                ui.label(pick(ctx.lang, "（忽略尾部设置）", "(tail ignored)"));
            }

            if ui.button(pick(ctx.lang, "手动加载", "Load")).clicked() {
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
                // Next frame will load (auto_load_transcript=true) or user can click again.
                if !ctx.view.history.auto_load_transcript {
                    ctx.view.history.auto_load_transcript = true;
                }
            }

            if ui.button(pick(ctx.lang, "打开文件", "Open file")).clicked() {
                let path = ctx.view.history.sessions[selected_idx].path.clone();
                if let Err(e) = open_in_file_manager(&path, true) {
                    *ctx.last_error = Some(format!("open session failed: {e}"));
                }
            }

            if ui.button(pick(ctx.lang, "复制全部", "Copy all")).clicked() {
                let mut out = String::new();
                for msg in ctx.view.history.transcript_messages.iter() {
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    out.push_str(&format!("[{ts}] {role}:\n"));
                    out.push_str(msg.text.as_str());
                    out.push_str("\n\n");
                }
                ui.ctx().copy_text(out);
                *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
            }

            if ui
                .button(pick(ctx.lang, "复制当前消息", "Copy selected"))
                .clicked()
            {
                let total = ctx.view.history.transcript_messages.len();
                if total > 0 {
                    let idx = ctx
                        .view
                        .history
                        .transcript_selected_msg_idx
                        .min(total.saturating_sub(1));
                    let msg = &ctx.view.history.transcript_messages[idx];
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    let out = format!("[{ts}] {role}:\n{}\n", msg.text);
                    ui.ctx().copy_text(out);
                    *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
                }
            }

            if ui.button(pick(ctx.lang, "首条", "First")).clicked() {
                ctx.view.history.transcript_view = TranscriptViewMode::Messages;
                ctx.view.history.transcript_selected_msg_idx = 0;
                ctx.view.history.transcript_scroll_to_msg_idx = Some(0);
            }

            if ui.button(pick(ctx.lang, "末条", "Last")).clicked() {
                let total = ctx.view.history.transcript_messages.len();
                if total > 0 {
                    let last = total.saturating_sub(1);
                    ctx.view.history.transcript_view = TranscriptViewMode::Messages;
                    ctx.view.history.transcript_selected_msg_idx = last;
                    ctx.view.history.transcript_scroll_to_msg_idx = Some(last);
                }
            }
        });

        if let Some(err) = ctx.view.history.transcript_error.as_deref() {
            cols[1].colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        }

        history_transcript::render_transcript_body(
            &mut cols[1],
            ctx.lang,
            &mut ctx.view.history,
            480.0,
        );
    });
}

fn render_history_vertical(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let max_h = ui.available_height();
    let desired_h = ctx
        .view
        .history
        .sessions_panel_height
        .clamp(160.0, max_h * 0.55);

    let mut pending_select: Option<(usize, String)> = None;

    let resp = egui::TopBottomPanel::top("history_vertical_sessions_panel")
        .resizable(true)
        .default_height(desired_h)
        .min_height(160.0)
        .max_height(max_h * 0.8)
        .show_inside(ui, |ui| {
            pending_select = history_sessions::render_sessions_panel_vertical(ui, ctx);
        });

    ctx.view.history.sessions_panel_height = resp.response.rect.height();
    let pointer_down = ui.ctx().input(|i| i.pointer.any_down());
    if !pointer_down
        && (ctx.gui_cfg.history.sessions_panel_height - ctx.view.history.sessions_panel_height)
            .abs()
            > 2.0
    {
        ctx.gui_cfg.history.sessions_panel_height = ctx.view.history.sessions_panel_height;
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        }
    }

    if let Some((idx, id)) = pending_select.take() {
        ctx.view.history.selected_idx = idx;
        ctx.view.history.selected_id = Some(id);
        ctx.view.history.loaded_for = None;
        cancel_transcript_load(&mut ctx.view.history);
        ctx.view.history.transcript_raw_messages.clear();
        ctx.view.history.transcript_messages.clear();
        ctx.view.history.transcript_error = None;
        ctx.view.history.transcript_plain_key = None;
        ctx.view.history.transcript_plain_text.clear();
        ctx.view.history.transcript_selected_msg_idx = 0;
    }

    if ctx.view.history.auto_load_transcript
        && let Some(id) = ctx.view.history.selected_id.clone()
    {
        let selected_idx = ctx
            .view
            .history
            .selected_idx
            .min(ctx.view.history.sessions.len().saturating_sub(1));
        let path = ctx.view.history.sessions[selected_idx].path.clone();
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        ensure_transcript_loading(ctx, path, (id, tail));
    }

    ui.add_space(6.0);

    ui.heading(pick(ctx.lang, "对话记录", "Transcript"));
    ui.add_space(4.0);

    let selected_idx = ctx
        .view
        .history
        .selected_idx
        .min(ctx.view.history.sessions.len().saturating_sub(1));
    let selected = &ctx.view.history.sessions[selected_idx];
    let selected_id = selected.id.clone();
    let selected_cwd = selected.cwd.clone().unwrap_or_else(|| "-".to_string());
    let selected_path = selected.path.clone();
    let workdir = history_workdir_from_cwd(selected_cwd.as_str(), ctx.view.history.infer_git_root);
    let resume_cmd = {
        let t = ctx.view.history.resume_cmd.trim();
        if t.is_empty() {
            format!("codex resume {selected_id}")
        } else if t.contains("{id}") {
            t.replace("{id}", &selected_id)
        } else {
            format!("{t} {selected_id}")
        }
    };

    ui.horizontal(|ui| {
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
            && let Err(e) = open_in_file_manager(&selected_path, true)
        {
            *ctx.last_error = Some(format!("open session failed: {e}"));
        }

        let n = ctx.view.history.batch_selected_ids.len();
        let label = match ctx.lang {
            Language::Zh => format!("在 wt 中打开选中({n})"),
            Language::En => format!("Open selected in wt ({n})"),
        };
        let can_open = cfg!(windows) && n > 0;
        if ui.add_enabled(can_open, egui::Button::new(label)).clicked() {
            let items = ctx
                .view
                .history
                .sessions
                .iter()
                .filter(|s| ctx.view.history.batch_selected_ids.contains(&s.id))
                .filter_map(|s| {
                    let cwd = s.cwd.as_deref()?.trim().to_string();
                    if cwd.is_empty() || cwd == "-" {
                        return None;
                    }
                    let workdir =
                        history_workdir_from_cwd(cwd.as_str(), ctx.view.history.infer_git_root);
                    if !std::path::Path::new(workdir.as_str()).exists() {
                        return None;
                    }
                    let sid = s.id.clone();
                    let cmd = {
                        let t = ctx.view.history.resume_cmd.trim();
                        if t.is_empty() {
                            format!("codex resume {sid}")
                        } else if t.contains("{id}") {
                            t.replace("{id}", &sid)
                        } else {
                            format!("{t} {sid}")
                        }
                    };
                    Some((workdir, cmd))
                })
                .collect::<Vec<_>>();

            if items.is_empty() {
                *ctx.last_error = Some(
                    pick(
                        ctx.lang,
                        "没有可打开的会话（cwd 不可用或目录不存在）",
                        "No sessions to open (cwd unavailable or missing)",
                    )
                    .to_string(),
                );
            } else {
                open_wt_items(ctx, items);
            }
        }
    });

    ui.label(format!("id: {}", selected_id));
    ui.label(format!("dir: {}", workdir));

    ui.horizontal(|ui| {
        let mut hide = ctx.view.history.hide_tool_calls;
        ui.checkbox(&mut hide, pick(ctx.lang, "隐藏工具调用", "Hide tool calls"));
        if hide != ctx.view.history.hide_tool_calls {
            ctx.view.history.hide_tool_calls = hide;
            ctx.view.history.transcript_messages = history_transcript::filter_tool_calls(
                ctx.view.history.transcript_raw_messages.clone(),
                hide,
            );
            ctx.view.history.transcript_plain_key = None;
            ctx.view.history.transcript_plain_text.clear();
        }

        ui.label(pick(ctx.lang, "显示", "View"));
        egui::ComboBox::from_id_salt("history_transcript_view_vertical")
            .selected_text(match ctx.view.history.transcript_view {
                TranscriptViewMode::Messages => pick(ctx.lang, "消息列表", "Messages"),
                TranscriptViewMode::PlainText => pick(ctx.lang, "纯文本", "Plain text"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut ctx.view.history.transcript_view,
                    TranscriptViewMode::Messages,
                    pick(ctx.lang, "消息列表", "Messages"),
                );
                ui.selectable_value(
                    &mut ctx.view.history.transcript_view,
                    TranscriptViewMode::PlainText,
                    pick(ctx.lang, "纯文本", "Plain text"),
                );
            });

        let mut full = ctx.view.history.transcript_full;
        ui.checkbox(&mut full, pick(ctx.lang, "全量", "All"));
        if full != ctx.view.history.transcript_full {
            ctx.view.history.transcript_full = full;
            ctx.view.history.loaded_for = None;
            cancel_transcript_load(&mut ctx.view.history);
        }

        ui.label(pick(ctx.lang, "尾部条数", "Tail"));
        ui.add_enabled(
            !ctx.view.history.transcript_full,
            egui::DragValue::new(&mut ctx.view.history.transcript_tail)
                .range(10..=500)
                .speed(1),
        );
        if ctx.view.history.transcript_full {
            ui.label(pick(ctx.lang, "（忽略尾部设置）", "(tail ignored)"));
        }

        if ui.button(pick(ctx.lang, "手动加载", "Load")).clicked() {
            ctx.view.history.loaded_for = None;
            cancel_transcript_load(&mut ctx.view.history);
            if !ctx.view.history.auto_load_transcript {
                ctx.view.history.auto_load_transcript = true;
            }
        }

        if ui.button(pick(ctx.lang, "复制全部", "Copy all")).clicked() {
            let mut out = String::new();
            for msg in ctx.view.history.transcript_messages.iter() {
                let ts = msg.timestamp.as_deref().unwrap_or("-");
                let role = msg.role.as_str();
                out.push_str(&format!("[{ts}] {role}:\n"));
                out.push_str(msg.text.as_str());
                out.push_str("\n\n");
            }
            ui.ctx().copy_text(out);
            *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
        }

        if ui
            .button(pick(ctx.lang, "复制当前消息", "Copy selected"))
            .clicked()
        {
            let total = ctx.view.history.transcript_messages.len();
            if total > 0 {
                let idx = ctx
                    .view
                    .history
                    .transcript_selected_msg_idx
                    .min(total.saturating_sub(1));
                let msg = &ctx.view.history.transcript_messages[idx];
                let ts = msg.timestamp.as_deref().unwrap_or("-");
                let role = msg.role.as_str();
                let out = format!("[{ts}] {role}:\n{}\n", msg.text);
                ui.ctx().copy_text(out);
                *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
            }
        }

        if ui.button(pick(ctx.lang, "首条", "First")).clicked() {
            ctx.view.history.transcript_view = TranscriptViewMode::Messages;
            ctx.view.history.transcript_selected_msg_idx = 0;
            ctx.view.history.transcript_scroll_to_msg_idx = Some(0);
        }

        if ui.button(pick(ctx.lang, "末条", "Last")).clicked() {
            let total = ctx.view.history.transcript_messages.len();
            if total > 0 {
                let last = total.saturating_sub(1);
                ctx.view.history.transcript_view = TranscriptViewMode::Messages;
                ctx.view.history.transcript_selected_msg_idx = last;
                ctx.view.history.transcript_scroll_to_msg_idx = Some(last);
            }
        }
    });

    if let Some(err) = ctx.view.history.transcript_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }

    let transcript_max_h = ui.available_height();
    history_transcript::render_transcript_body(
        ui,
        ctx.lang,
        &mut ctx.view.history,
        transcript_max_h,
    );
}

fn render_history_all_by_date(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(260.0)
                .hint_text(pick(
                    ctx.lang,
                    "关键词（匹配 cwd 或首条用户消息）",
                    "keyword (cwd or first user message)",
                )),
        );

        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            let limit = ctx.view.history.all_days_limit;
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_session_day_dirs(limit))
            {
                Ok(dates) => {
                    ctx.view.history.all_dates = dates;
                    ctx.view.history.last_error = None;
                    ctx.view.history.loaded_day_for = None;
                    ctx.view.history.all_day_sessions.clear();
                    ctx.view.history.selected_id = None;
                    cancel_transcript_load(&mut ctx.view.history);
                    ctx.view.history.transcript_raw_messages.clear();
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = None;
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        if ui
            .button(pick(ctx.lang, "加载更多天", "Load more days"))
            .clicked()
        {
            ctx.view.history.all_days_limit = ctx.view.history.all_days_limit.saturating_add(120);
            let limit = ctx.view.history.all_days_limit;
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_session_day_dirs(limit))
            {
                Ok(dates) => {
                    ctx.view.history.all_dates = dates;
                    ctx.view.history.last_error = None;
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        ui.checkbox(
            &mut ctx.view.history.auto_load_transcript,
            pick(ctx.lang, "自动加载对话", "Auto load transcript"),
        );
    });

    if let Some(err) = ctx.view.history.last_error.as_deref() {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }

    if ctx.view.history.all_dates.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "暂无日期索引。点击“刷新”加载。",
            "No date index loaded. Click Refresh.",
        ));
        return;
    }

    // Keep selected date stable.
    if ctx
        .view
        .history
        .all_selected_date
        .as_deref()
        .is_none_or(|d| !ctx.view.history.all_dates.iter().any(|x| x.date == d))
    {
        ctx.view.history.all_selected_date = Some(ctx.view.history.all_dates[0].date.clone());
        ctx.view.history.loaded_day_for = None;
    }

    // Auto-load day sessions when date changes.
    if let Some(date) = ctx.view.history.all_selected_date.clone()
        && ctx.view.history.loaded_day_for.as_deref() != Some(date.as_str())
    {
        let limit = ctx.view.history.all_day_limit;
        let day_dir = ctx
            .view
            .history
            .all_dates
            .iter()
            .find(|x| x.date == date)
            .map(|x| x.path.clone());
        if let Some(day_dir) = day_dir {
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_sessions_in_day_dir(
                    &day_dir, limit,
                )) {
                Ok(mut list) => {
                    list.sort_by_key(|s| std::cmp::Reverse(s.mtime_ms));
                    ctx.view.history.all_day_sessions = list;
                    ctx.view.history.loaded_day_for = Some(date.clone());
                    ctx.view.history.selected_id = None;
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = None;
                    ctx.view.history.loaded_for = None;
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }
    }

    let q = ctx.view.history.query.trim().to_lowercase();

    ui.add_space(6.0);
    let layout =
        resolve_history_layout(ctx.view.history.layout_mode.as_str(), ui.available_width());
    if layout == ResolvedHistoryLayout::Vertical {
        render_history_all_by_date_vertical(ui, ctx, q.as_str());
        return;
    }
    ui.columns(3, |cols| {
        cols[0].heading(pick(ctx.lang, "日期", "Dates"));
        cols[0].add_space(4.0);
        {
            let total = ctx.view.history.all_dates.len();
            let row_h = 22.0;
            egui::ScrollArea::vertical()
                .id_salt("history_all_by_date_dates_scroll")
                .max_height(520.0)
                .show_rows(&mut cols[0], row_h, total, |ui, range| {
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

        cols[1].heading(pick(ctx.lang, "会话", "Sessions"));
        cols[1].add_space(4.0);
        let mut action_batch_select_visible = false;
        let mut action_batch_clear = false;
        cols[1].horizontal(|ui| {
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
        cols[1].add_space(4.0);

        let mut visible_indices: Vec<usize> = Vec::new();
        for (idx, s) in ctx.view.history.all_day_sessions.iter().enumerate() {
            if q.is_empty() {
                visible_indices.push(idx);
                continue;
            }
            let mut matched = false;
            if let Some(cwd) = s.cwd.as_deref() {
                matched |= cwd.to_lowercase().contains(q.as_str());
            }
            if let Some(msg) = s.first_user_message.as_deref() {
                matched |= msg.to_lowercase().contains(q.as_str());
            }
            if matched {
                visible_indices.push(idx);
            }
        }

        {
            let total = visible_indices.len();
            let row_h = 22.0;
            egui::ScrollArea::vertical()
                .id_salt("history_all_by_date_sessions_scroll")
                .max_height(520.0)
                .show_rows(&mut cols[1], row_h, total, |ui, range| {
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
                                ctx.view.history.selected_id = Some(sid.clone());
                                ctx.view.history.selected_idx = idx;
                                ctx.view.history.loaded_for = None;
                                cancel_transcript_load(&mut ctx.view.history);
                                ctx.view.history.transcript_raw_messages.clear();
                                ctx.view.history.transcript_messages.clear();
                                ctx.view.history.transcript_error = None;
                                ctx.view.history.transcript_plain_key = None;
                                ctx.view.history.transcript_plain_text.clear();
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

        cols[2].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[2].add_space(4.0);

        let selected_idx = ctx.view.history.selected_id.as_deref().and_then(|id| {
            ctx.view
                .history
                .all_day_sessions
                .iter()
                .position(|s| s.id == id)
        });
        let selected = selected_idx.and_then(|idx| ctx.view.history.all_day_sessions.get(idx));

        if selected.is_none() {
            cols[2].label(pick(
                ctx.lang,
                "选择一个会话以预览对话。",
                "Select a session to preview.",
            ));
            return;
        }
        let selected = selected.unwrap();
        let selected_id = selected.id.clone();
        let selected_cwd = selected.cwd.clone().unwrap_or_else(|| "-".to_string());

        let workdir =
            history_workdir_from_cwd(selected_cwd.as_str(), ctx.view.history.infer_git_root);

        let resume_cmd = {
            let t = ctx.view.history.resume_cmd.trim();
            if t.is_empty() {
                format!("codex resume {selected_id}")
            } else if t.contains("{id}") {
                t.replace("{id}", &selected_id)
            } else {
                format!("{t} {selected_id}")
            }
        };

        cols[2].group(|ui| {
            ui.label(pick(ctx.lang, "恢复", "Resume"));

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "命令模板", "Template"));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.resume_cmd)
                        .desired_width(260.0),
                );
                if resp.lost_focus()
                    && ctx.gui_cfg.history.resume_cmd != ctx.view.history.resume_cmd
                {
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
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut ctx.view.history.shell).desired_width(140.0),
                );
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
                egui::ComboBox::from_id_salt("history_wt_batch_mode_all_by_date")
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

                let n = ctx.view.history.batch_selected_ids.len();
                let label = match ctx.lang {
                    Language::Zh => format!("在 wt 中打开选中({n})"),
                    Language::En => format!("Open selected in wt ({n})"),
                };
                let can_open = cfg!(windows) && n > 0;
                if ui.add_enabled(can_open, egui::Button::new(label)).clicked() {
                    let items = ctx
                        .view
                        .history
                        .all_day_sessions
                        .iter()
                        .filter(|s| ctx.view.history.batch_selected_ids.contains(&s.id))
                        .filter_map(|s| {
                            let cwd = s.cwd.as_deref()?.trim().to_string();
                            if cwd.is_empty() || cwd == "-" {
                                return None;
                            }
                            let workdir = history_workdir_from_cwd(
                                cwd.as_str(),
                                ctx.view.history.infer_git_root,
                            );
                            if !std::path::Path::new(workdir.as_str()).exists() {
                                return None;
                            }
                            let sid = s.id.clone();
                            let cmd = {
                                let t = ctx.view.history.resume_cmd.trim();
                                if t.is_empty() {
                                    format!("codex resume {sid}")
                                } else if t.contains("{id}") {
                                    t.replace("{id}", &sid)
                                } else {
                                    format!("{t} {sid}")
                                }
                            };
                            Some((workdir, cmd))
                        })
                        .collect::<Vec<_>>();

                    if items.is_empty() {
                        *ctx.last_error = Some(
                            pick(
                                ctx.lang,
                                "没有可打开的会话（cwd 不可用或目录不存在）",
                                "No sessions to open (cwd unavailable or missing)",
                            )
                            .to_string(),
                        );
                    } else {
                        let mode = ctx
                            .gui_cfg
                            .history
                            .wt_batch_mode
                            .trim()
                            .to_ascii_lowercase();
                        let shell = ctx.view.history.shell.trim();
                        let keep_open = ctx.view.history.keep_open;

                        let result = if mode == "windows" {
                            let mut last_err: Option<anyhow::Error> = None;
                            for (cwd, cmd) in items.iter() {
                                if let Err(e) = spawn_windows_terminal_wt_new_tab(
                                    -1,
                                    cwd.as_str(),
                                    shell,
                                    keep_open,
                                    cmd.as_str(),
                                ) {
                                    last_err = Some(e);
                                    break;
                                }
                            }
                            match last_err {
                                Some(e) => Err(e),
                                None => Ok(()),
                            }
                        } else {
                            spawn_windows_terminal_wt_tabs_in_one_window(&items, shell, keep_open)
                        };

                        match result {
                            Ok(()) => {
                                *ctx.last_info = Some(
                                    pick(
                                        ctx.lang,
                                        "已启动 Windows Terminal",
                                        "Started Windows Terminal",
                                    )
                                    .to_string(),
                                );
                            }
                            Err(e) => *ctx.last_error = Some(format!("spawn wt failed: {e}")),
                        }
                    }
                }
            });

            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "复制 root+id", "Copy root+id"))
                    .clicked()
                {
                    if workdir.trim().is_empty() || workdir == "-" {
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
                    } else {
                        ui.ctx()
                            .copy_text(format!("{} {}", workdir.trim(), selected_id));
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
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
                        *ctx.last_error =
                            Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
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
                    && let Err(e) = open_in_file_manager(&selected.path, true)
                {
                    *ctx.last_error = Some(format!("open session failed: {e}"));
                }
            });

            ui.label(format!("id: {}", selected_id));
            ui.label(format!("dir: {}", workdir));

            if let Some(first) = selected.first_user_message.as_deref() {
                let mut text = first.to_string();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            }
        });

        if ctx.view.history.auto_load_transcript {
            let tail = if ctx.view.history.transcript_full {
                None
            } else {
                Some(ctx.view.history.transcript_tail)
            };
            let key = (selected_id.clone(), tail);
            ensure_transcript_loading(ctx, selected.path.clone(), key);
        }

        cols[2].add_space(6.0);
        cols[2].horizontal(|ui| {
            let mut hide = ctx.view.history.hide_tool_calls;
            ui.checkbox(&mut hide, pick(ctx.lang, "隐藏工具调用", "Hide tool calls"));
            if hide != ctx.view.history.hide_tool_calls {
                ctx.view.history.hide_tool_calls = hide;
                ctx.view.history.transcript_messages = history_transcript::filter_tool_calls(
                    ctx.view.history.transcript_raw_messages.clone(),
                    hide,
                );
                ctx.view.history.transcript_plain_key = None;
                ctx.view.history.transcript_plain_text.clear();
            }

            ui.label(pick(ctx.lang, "显示", "View"));
            egui::ComboBox::from_id_salt("history_transcript_view_all")
                .selected_text(match ctx.view.history.transcript_view {
                    TranscriptViewMode::Messages => pick(ctx.lang, "消息列表", "Messages"),
                    TranscriptViewMode::PlainText => pick(ctx.lang, "纯文本", "Plain text"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::Messages,
                        pick(ctx.lang, "消息列表", "Messages"),
                    );
                    ui.selectable_value(
                        &mut ctx.view.history.transcript_view,
                        TranscriptViewMode::PlainText,
                        pick(ctx.lang, "纯文本", "Plain text"),
                    );
                });
            let mut full = ctx.view.history.transcript_full;
            ui.checkbox(&mut full, pick(ctx.lang, "全量", "All"));
            if full != ctx.view.history.transcript_full {
                ctx.view.history.transcript_full = full;
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
            }

            ui.label(pick(ctx.lang, "尾部条数", "Tail"));
            ui.add_enabled(
                !ctx.view.history.transcript_full,
                egui::DragValue::new(&mut ctx.view.history.transcript_tail)
                    .range(10..=500)
                    .speed(1),
            );
            if ctx.view.history.transcript_full {
                ui.label(pick(ctx.lang, "（忽略尾部设置）", "(tail ignored)"));
            }

            if ui.button(pick(ctx.lang, "手动加载", "Load")).clicked() {
                ctx.view.history.loaded_for = None;
                cancel_transcript_load(&mut ctx.view.history);
                if !ctx.view.history.auto_load_transcript {
                    ctx.view.history.auto_load_transcript = true;
                }
            }

            if ui.button(pick(ctx.lang, "复制全部", "Copy all")).clicked() {
                let mut out = String::new();
                for msg in ctx.view.history.transcript_messages.iter() {
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    out.push_str(&format!("[{ts}] {role}:\n"));
                    out.push_str(msg.text.as_str());
                    out.push_str("\n\n");
                }
                ui.ctx().copy_text(out);
                *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
            }

            if ui
                .button(pick(ctx.lang, "复制当前消息", "Copy selected"))
                .clicked()
            {
                let total = ctx.view.history.transcript_messages.len();
                if total > 0 {
                    let idx = ctx
                        .view
                        .history
                        .transcript_selected_msg_idx
                        .min(total.saturating_sub(1));
                    let msg = &ctx.view.history.transcript_messages[idx];
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    let out = format!("[{ts}] {role}:\n{}\n", msg.text);
                    ui.ctx().copy_text(out);
                    *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
                }
            }

            if ui.button(pick(ctx.lang, "首条", "First")).clicked() {
                ctx.view.history.transcript_view = TranscriptViewMode::Messages;
                ctx.view.history.transcript_selected_msg_idx = 0;
                ctx.view.history.transcript_scroll_to_msg_idx = Some(0);
            }

            if ui.button(pick(ctx.lang, "末条", "Last")).clicked() {
                let total = ctx.view.history.transcript_messages.len();
                if total > 0 {
                    let last = total.saturating_sub(1);
                    ctx.view.history.transcript_view = TranscriptViewMode::Messages;
                    ctx.view.history.transcript_selected_msg_idx = last;
                    ctx.view.history.transcript_scroll_to_msg_idx = Some(last);
                }
            }
        });

        if let Some(err) = ctx.view.history.transcript_error.as_deref() {
            cols[2].colored_label(egui::Color32::from_rgb(200, 120, 40), err);
        }

        history_transcript::render_transcript_body(
            &mut cols[2],
            ctx.lang,
            &mut ctx.view.history,
            360.0,
        );
    });
}

fn render_history_all_by_date_vertical(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, q: &str) {
    let max_h = ui.available_height();
    let desired_h = ctx
        .view
        .history
        .sessions_panel_height
        .clamp(200.0, max_h * 0.55);

    let q = q.trim();
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

    let mut action_batch_select_visible = false;
    let mut action_batch_clear = false;

    let resp = egui::TopBottomPanel::top("history_all_vertical_nav_panel")
        .resizable(true)
        .default_height(desired_h)
        .min_height(200.0)
        .max_height(max_h * 0.8)
        .show_inside(ui, |ui| {
            ui.columns(2, |cols| {
                cols[0].heading(pick(ctx.lang, "日期", "Dates"));
                cols[0].add_space(4.0);
                {
                    let total = ctx.view.history.all_dates.len();
                    let row_h = 22.0;
                    let max_h = cols[0].available_height().max(160.0);
                    egui::ScrollArea::vertical()
                        .id_salt("history_all_by_date_dates_scroll_vertical")
                        .max_height(max_h)
                        .show_rows(&mut cols[0], row_h, total, |ui, range| {
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

                cols[1].heading(pick(ctx.lang, "会话", "Sessions"));
                cols[1].add_space(4.0);
                cols[1].horizontal(|ui| {
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
                cols[1].add_space(4.0);

                let total = visible_indices.len();
                let row_h = 22.0;
                let max_h = cols[1].available_height().max(160.0);
                egui::ScrollArea::vertical()
                    .id_salt("history_all_by_date_sessions_scroll_vertical")
                    .max_height(max_h)
                    .show_rows(&mut cols[1], row_h, total, |ui, range| {
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
                                shorten(&root_or_cwd, 32),
                                id_short,
                                shorten(t, 19),
                                shorten(first, 40)
                            );

                            let sid = s.id.clone();
                            ui.horizontal(|ui| {
                                let mut checked =
                                    ctx.view.history.batch_selected_ids.contains(&sid);
                                if ui.checkbox(&mut checked, "").changed() {
                                    if checked {
                                        ctx.view.history.batch_selected_ids.insert(sid.clone());
                                    } else {
                                        ctx.view.history.batch_selected_ids.remove(&sid);
                                    }
                                }

                                if ui.selectable_label(selected, label).clicked() {
                                    ctx.view.history.selected_id = Some(sid.clone());
                                    cancel_transcript_load(&mut ctx.view.history);
                                    ctx.view.history.transcript_raw_messages.clear();
                                    ctx.view.history.transcript_messages.clear();
                                    ctx.view.history.transcript_error = None;
                                    ctx.view.history.loaded_for = None;
                                    ctx.view.history.transcript_plain_key = None;
                                    ctx.view.history.transcript_plain_text.clear();
                                    ctx.view.history.transcript_selected_msg_idx = 0;
                                }
                            });
                        }
                    });
            });
        });

    ctx.view.history.sessions_panel_height = resp.response.rect.height();
    let pointer_down = ui.ctx().input(|i| i.pointer.any_down());
    if !pointer_down
        && (ctx.gui_cfg.history.sessions_panel_height - ctx.view.history.sessions_panel_height)
            .abs()
            > 2.0
    {
        ctx.gui_cfg.history.sessions_panel_height = ctx.view.history.sessions_panel_height;
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        }
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

    ui.add_space(6.0);
    ui.heading(pick(ctx.lang, "对话记录", "Transcript"));
    ui.add_space(4.0);

    let selected_idx = ctx.view.history.selected_id.as_deref().and_then(|id| {
        ctx.view
            .history
            .all_day_sessions
            .iter()
            .position(|s| s.id == id)
    });
    let selected = selected_idx.and_then(|idx| ctx.view.history.all_day_sessions.get(idx));
    if selected.is_none() {
        ui.label(pick(
            ctx.lang,
            "选择一个会话以预览对话。",
            "Select a session to preview.",
        ));
        return;
    }
    let (selected_id, selected_cwd, selected_path) = {
        let s = selected.unwrap();
        (
            s.id.clone(),
            s.cwd.clone().unwrap_or_else(|| "-".to_string()),
            s.path.clone(),
        )
    };

    let workdir = history_workdir_from_cwd(selected_cwd.as_str(), ctx.view.history.infer_git_root);
    let resume_cmd = {
        let t = ctx.view.history.resume_cmd.trim();
        if t.is_empty() {
            format!("codex resume {selected_id}")
        } else if t.contains("{id}") {
            t.replace("{id}", &selected_id)
        } else {
            format!("{t} {selected_id}")
        }
    };

    if ctx.view.history.auto_load_transcript {
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        let key = (selected_id.clone(), tail);
        ensure_transcript_loading(ctx, selected_path.clone(), key);
    }

    ui.horizontal(|ui| {
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
            && let Err(e) = open_in_file_manager(&selected_path, true)
        {
            *ctx.last_error = Some(format!("open session failed: {e}"));
        }

        let n = ctx.view.history.batch_selected_ids.len();
        let label = match ctx.lang {
            Language::Zh => format!("在 wt 中打开选中({n})"),
            Language::En => format!("Open selected in wt ({n})"),
        };
        let can_open = cfg!(windows) && n > 0;
        if ui.add_enabled(can_open, egui::Button::new(label)).clicked() {
            let items = ctx
                .view
                .history
                .all_day_sessions
                .iter()
                .filter(|s| ctx.view.history.batch_selected_ids.contains(&s.id))
                .filter_map(|s| {
                    let cwd = s.cwd.as_deref()?.trim().to_string();
                    if cwd.is_empty() || cwd == "-" {
                        return None;
                    }
                    let workdir =
                        history_workdir_from_cwd(cwd.as_str(), ctx.view.history.infer_git_root);
                    if !std::path::Path::new(workdir.as_str()).exists() {
                        return None;
                    }
                    let sid = s.id.clone();
                    let cmd = {
                        let t = ctx.view.history.resume_cmd.trim();
                        if t.is_empty() {
                            format!("codex resume {sid}")
                        } else if t.contains("{id}") {
                            t.replace("{id}", &sid)
                        } else {
                            format!("{t} {sid}")
                        }
                    };
                    Some((workdir, cmd))
                })
                .collect::<Vec<_>>();

            if items.is_empty() {
                *ctx.last_error = Some(
                    pick(
                        ctx.lang,
                        "没有可打开的会话（cwd 不可用或目录不存在）",
                        "No sessions to open (cwd unavailable or missing)",
                    )
                    .to_string(),
                );
            } else {
                let mode = ctx
                    .gui_cfg
                    .history
                    .wt_batch_mode
                    .trim()
                    .to_ascii_lowercase();
                let shell = ctx.view.history.shell.trim();
                let keep_open = ctx.view.history.keep_open;

                let result = if mode == "windows" {
                    let mut last_err: Option<anyhow::Error> = None;
                    for (cwd, cmd) in items.iter() {
                        if let Err(e) = spawn_windows_terminal_wt_new_tab(
                            -1,
                            cwd.as_str(),
                            shell,
                            keep_open,
                            cmd.as_str(),
                        ) {
                            last_err = Some(e);
                            break;
                        }
                    }
                    match last_err {
                        Some(e) => Err(e),
                        None => Ok(()),
                    }
                } else {
                    spawn_windows_terminal_wt_tabs_in_one_window(&items, shell, keep_open)
                };

                match result {
                    Ok(()) => {
                        *ctx.last_info = Some(
                            pick(
                                ctx.lang,
                                "已启动 Windows Terminal",
                                "Started Windows Terminal",
                            )
                            .to_string(),
                        );
                    }
                    Err(e) => *ctx.last_error = Some(format!("spawn wt failed: {e}")),
                }
            }
        }
    });

    ui.label(format!("id: {}", selected_id));
    ui.label(format!("dir: {}", workdir));

    ui.horizontal(|ui| {
        let mut hide = ctx.view.history.hide_tool_calls;
        ui.checkbox(&mut hide, pick(ctx.lang, "隐藏工具调用", "Hide tool calls"));
        if hide != ctx.view.history.hide_tool_calls {
            ctx.view.history.hide_tool_calls = hide;
            ctx.view.history.transcript_messages = history_transcript::filter_tool_calls(
                ctx.view.history.transcript_raw_messages.clone(),
                hide,
            );
            ctx.view.history.transcript_plain_key = None;
            ctx.view.history.transcript_plain_text.clear();
        }

        ui.label(pick(ctx.lang, "显示", "View"));
        egui::ComboBox::from_id_salt("history_transcript_view_all_vertical")
            .selected_text(match ctx.view.history.transcript_view {
                TranscriptViewMode::Messages => pick(ctx.lang, "消息列表", "Messages"),
                TranscriptViewMode::PlainText => pick(ctx.lang, "纯文本", "Plain text"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut ctx.view.history.transcript_view,
                    TranscriptViewMode::Messages,
                    pick(ctx.lang, "消息列表", "Messages"),
                );
                ui.selectable_value(
                    &mut ctx.view.history.transcript_view,
                    TranscriptViewMode::PlainText,
                    pick(ctx.lang, "纯文本", "Plain text"),
                );
            });
        let mut full = ctx.view.history.transcript_full;
        ui.checkbox(&mut full, pick(ctx.lang, "全量", "All"));
        if full != ctx.view.history.transcript_full {
            ctx.view.history.transcript_full = full;
            ctx.view.history.loaded_for = None;
            cancel_transcript_load(&mut ctx.view.history);
        }

        ui.label(pick(ctx.lang, "尾部条数", "Tail"));
        ui.add_enabled(
            !ctx.view.history.transcript_full,
            egui::DragValue::new(&mut ctx.view.history.transcript_tail)
                .range(10..=500)
                .speed(1),
        );
        if ctx.view.history.transcript_full {
            ui.label(pick(ctx.lang, "（忽略尾部设置）", "(tail ignored)"));
        }

        if ui.button(pick(ctx.lang, "手动加载", "Load")).clicked() {
            ctx.view.history.loaded_for = None;
            cancel_transcript_load(&mut ctx.view.history);
            if !ctx.view.history.auto_load_transcript {
                ctx.view.history.auto_load_transcript = true;
            }
        }

        if ui.button(pick(ctx.lang, "复制全部", "Copy all")).clicked() {
            let mut out = String::new();
            for msg in ctx.view.history.transcript_messages.iter() {
                let ts = msg.timestamp.as_deref().unwrap_or("-");
                let role = msg.role.as_str();
                out.push_str(&format!("[{ts}] {role}:\n"));
                out.push_str(msg.text.as_str());
                out.push_str("\n\n");
            }
            ui.ctx().copy_text(out);
            *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
        }

        if ui
            .button(pick(ctx.lang, "复制当前消息", "Copy selected"))
            .clicked()
        {
            let total = ctx.view.history.transcript_messages.len();
            if total > 0 {
                let idx = ctx
                    .view
                    .history
                    .transcript_selected_msg_idx
                    .min(total.saturating_sub(1));
                let msg = &ctx.view.history.transcript_messages[idx];
                let ts = msg.timestamp.as_deref().unwrap_or("-");
                let role = msg.role.as_str();
                let out = format!("[{ts}] {role}:\n{}\n", msg.text);
                ui.ctx().copy_text(out);
                *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
            }
        }

        if ui.button(pick(ctx.lang, "首条", "First")).clicked() {
            ctx.view.history.transcript_view = TranscriptViewMode::Messages;
            ctx.view.history.transcript_selected_msg_idx = 0;
            ctx.view.history.transcript_scroll_to_msg_idx = Some(0);
        }

        if ui.button(pick(ctx.lang, "末条", "Last")).clicked() {
            let total = ctx.view.history.transcript_messages.len();
            if total > 0 {
                let last = total.saturating_sub(1);
                ctx.view.history.transcript_view = TranscriptViewMode::Messages;
                ctx.view.history.transcript_selected_msg_idx = last;
                ctx.view.history.transcript_scroll_to_msg_idx = Some(last);
            }
        }
    });

    if let Some(err) = ctx.view.history.transcript_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }

    let transcript_max_h = ui.available_height();
    history_transcript::render_transcript_body(
        ui,
        ctx.lang,
        &mut ctx.view.history,
        transcript_max_h,
    );
}

fn cancel_transcript_load(state: &mut HistoryViewState) {
    if let Some(load) = state.transcript_load.take() {
        load.join.abort();
    }
}

fn poll_transcript_loader(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.history.transcript_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, res)) => {
            if seq != load.seq {
                ctx.view.history.transcript_load = None;
                return;
            }

            let key = load.key.clone();
            ctx.view.history.transcript_load = None;

            match res {
                Ok(msgs) => {
                    ctx.view.history.transcript_raw_messages = msgs;
                    ctx.view.history.transcript_messages = history_transcript::filter_tool_calls(
                        ctx.view.history.transcript_raw_messages.clone(),
                        ctx.view.history.hide_tool_calls,
                    );
                    ctx.view.history.transcript_error = None;
                    ctx.view.history.loaded_for = Some(key);
                    ctx.view.history.transcript_selected_msg_idx = 0;
                    ctx.view.history.transcript_scroll_to_msg_idx = Some(0);
                    ctx.view.history.transcript_plain_key = None;
                    ctx.view.history.transcript_plain_text.clear();
                }
                Err(e) => {
                    ctx.view.history.transcript_raw_messages.clear();
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = Some(e.to_string());
                    ctx.view.history.loaded_for = None;
                    ctx.view.history.transcript_scroll_to_msg_idx = None;
                    ctx.view.history.transcript_plain_key = None;
                    ctx.view.history.transcript_plain_text.clear();
                }
            }
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            ctx.view.history.transcript_load = None;
        }
    }
}

fn ensure_transcript_loading(
    ctx: &mut PageCtx<'_>,
    path: std::path::PathBuf,
    key: (String, Option<usize>),
) {
    if ctx.view.history.loaded_for.as_ref() == Some(&key) {
        return;
    }
    if let Some(load) = ctx.view.history.transcript_load.as_ref()
        && load.key == key
    {
        return;
    }

    start_transcript_load(ctx, path, key);
}

fn start_transcript_load(
    ctx: &mut PageCtx<'_>,
    path: std::path::PathBuf,
    key: (String, Option<usize>),
) {
    cancel_transcript_load(&mut ctx.view.history);

    ctx.view.history.transcript_load_seq = ctx.view.history.transcript_load_seq.saturating_add(1);
    let seq = ctx.view.history.transcript_load_seq;
    let tail = key.1;

    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let res = crate::sessions::read_codex_session_transcript(&path, tail).await;
        let _ = tx.send((seq, res));
    });

    ctx.view.history.transcript_load = Some(TranscriptLoad { seq, key, rx, join });
}
