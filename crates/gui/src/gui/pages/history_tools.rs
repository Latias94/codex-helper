use super::*;
use crate::gui::util::{
    spawn_windows_terminal_wt_new_tab, spawn_windows_terminal_wt_tabs_in_one_window,
};

pub(super) fn history_workdir_from_cwd(cwd: &str, infer_git_root: bool) -> String {
    let trimmed = cwd.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return "-".to_string();
    }
    if infer_git_root {
        crate::sessions::infer_project_root_from_cwd(trimmed)
    } else {
        trimmed.to_string()
    }
}

fn path_mtime_ms(path: &std::path::Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn session_summary_sort_key_ms(summary: &SessionSummary) -> u64 {
    summary
        .sort_hint_ms
        .unwrap_or_else(|| path_mtime_ms(summary.path.as_path()))
}

pub(super) fn sort_session_summaries_by_mtime_desc(list: &mut [SessionSummary]) {
    list.sort_by_key(|s| std::cmp::Reverse(session_summary_sort_key_ms(s)));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WtItemSkipReason {
    ObservedOnly,
    MissingCwd,
    InvalidWorkdir,
    WorkdirNotFound,
}

pub(super) fn workdir_status_from_cwd(
    cwd: Option<&str>,
    infer_git_root: bool,
) -> Result<String, WtItemSkipReason> {
    let Some(cwd) = cwd else {
        return Err(WtItemSkipReason::MissingCwd);
    };
    let cwd = cwd.trim();
    if cwd.is_empty() || cwd == "-" {
        return Err(WtItemSkipReason::MissingCwd);
    }

    let workdir = history_workdir_from_cwd(cwd, infer_git_root);
    let w = workdir.trim();
    if w.is_empty() || w == "-" {
        return Err(WtItemSkipReason::InvalidWorkdir);
    }
    if !std::path::Path::new(w).exists() {
        return Err(WtItemSkipReason::WorkdirNotFound);
    }
    Ok(workdir)
}

pub(super) fn workdir_status_from_summary(
    summary: &SessionSummary,
    infer_git_root: bool,
) -> Result<String, WtItemSkipReason> {
    if !matches!(summary.source, SessionSummarySource::LocalFile) {
        return Err(WtItemSkipReason::ObservedOnly);
    }
    workdir_status_from_cwd(summary.cwd.as_deref(), infer_git_root)
}

pub(super) fn build_wt_items_from_session_summaries<'a, I>(
    sessions: I,
    infer_git_root: bool,
    resume_cmd_template: &str,
) -> Vec<(String, String)>
where
    I: IntoIterator<Item = &'a SessionSummary>,
{
    let mut out = Vec::new();
    let t = resume_cmd_template.trim();
    for s in sessions {
        let Ok(workdir) = workdir_status_from_summary(s, infer_git_root) else {
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

pub(super) fn open_wt_items(ctx: &mut PageCtx<'_>, items: Vec<(String, String)>) {
    if !cfg!(windows) {
        *ctx.last_error = Some(pick(ctx.lang, "仅支持 Windows", "Windows only").to_string());
        return;
    }

    if items.is_empty() {
        *ctx.last_error = Some(
            pick(
                ctx.lang,
                "没有可打开的会话（cwd 不可用或目录不存在）",
                "No sessions to open (cwd unavailable or missing)",
            )
            .to_string(),
        );
        return;
    }

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
        for (cwd, cmd) in &items {
            if let Err(e) =
                spawn_windows_terminal_wt_new_tab(-1, cwd.as_str(), shell, keep_open, cmd.as_str())
            {
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
        Err(e) => {
            *ctx.last_error = Some(format!("spawn wt failed: {e}"));
        }
    }
}
