use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream;
use tokio::sync::mpsc;

use super::i18n::{self, msg};
use super::model::now_ms;
use super::state::{RecentCodexRow, UiState, merge_codex_history_external_focus};

#[derive(Debug)]
pub(super) struct CodexHistoryRefreshResult {
    generation: u64,
    result: Result<Vec<crate::sessions::SessionSummary>, String>,
}

#[derive(Debug)]
pub(super) struct CodexRecentRefreshResult {
    generation: u64,
    result: Result<CodexRecentRefreshPayload, String>,
}

#[derive(Debug)]
struct CodexRecentRefreshPayload {
    rows: Vec<RecentCodexRow>,
    branch_cache: HashMap<String, Option<String>>,
}

const CODEX_RECENT_BRANCH_LOOKUP_CONCURRENCY: usize = 8;

pub(super) fn start_codex_history_refresh(
    ui: &mut UiState,
    tx: mpsc::UnboundedSender<CodexHistoryRefreshResult>,
) {
    ui.codex_history_refresh_generation = ui.codex_history_refresh_generation.wrapping_add(1);
    let generation = ui.codex_history_refresh_generation;
    ui.codex_history_loading = true;
    ui.codex_history_error = None;
    ui.toast = Some((
        i18n::text(ui.language, msg::HISTORY_REFRESHING).to_string(),
        Instant::now(),
    ));

    tokio::spawn(async move {
        let result = crate::sessions::find_codex_sessions_for_current_dir(200)
            .await
            .map_err(|err| err.to_string());
        let _ = tx.send(CodexHistoryRefreshResult { generation, result });
    });
}

pub(super) fn apply_codex_history_refresh_result(
    ui: &mut UiState,
    result: CodexHistoryRefreshResult,
) -> bool {
    if result.generation != ui.codex_history_refresh_generation {
        return false;
    }

    ui.codex_history_loading = false;
    ui.codex_history_loaded_at_ms = Some(now_ms());
    match result.result {
        Ok(list) => {
            ui.codex_history_sessions = list;
            if let Some(focus) = ui.codex_history_external_focus.as_ref() {
                merge_codex_history_external_focus(&mut ui.codex_history_sessions, focus);
            }
            ui.codex_history_error = None;
            ui.sync_codex_history_selection();
            ui.toast = Some((
                i18n::format_history_loaded(ui.language, ui.codex_history_sessions.len()),
                Instant::now(),
            ));
        }
        Err(err) => {
            if let Some(focus) = ui.codex_history_external_focus.as_ref() {
                merge_codex_history_external_focus(&mut ui.codex_history_sessions, focus);
            }
            ui.codex_history_error = Some(err.clone());
            ui.sync_codex_history_selection();
            ui.toast = Some((
                i18n::format_history_load_failed(ui.language, &err),
                Instant::now(),
            ));
        }
    }
    true
}

pub(super) fn start_codex_recent_refresh(
    ui: &mut UiState,
    tx: mpsc::UnboundedSender<CodexRecentRefreshResult>,
) {
    ui.codex_recent_refresh_generation = ui.codex_recent_refresh_generation.wrapping_add(1);
    let generation = ui.codex_recent_refresh_generation;
    let raw_cwd = ui.codex_recent_raw_cwd;
    let branch_cache = ui.codex_recent_branch_cache.clone();
    ui.codex_recent_loading = true;
    ui.codex_recent_error = None;
    ui.toast = Some((
        i18n::text(ui.language, msg::RECENT_REFRESHING).to_string(),
        Instant::now(),
    ));

    tokio::spawn(async move {
        let result = load_codex_recent_rows(raw_cwd, branch_cache)
            .await
            .map_err(|err| err.to_string());
        let _ = tx.send(CodexRecentRefreshResult { generation, result });
    });
}

pub(super) fn apply_codex_recent_refresh_result(
    ui: &mut UiState,
    result: CodexRecentRefreshResult,
) -> bool {
    if result.generation != ui.codex_recent_refresh_generation {
        return false;
    }

    ui.codex_recent_loading = false;
    ui.codex_recent_loaded_at_ms = Some(now_ms());
    match result.result {
        Ok(payload) => {
            ui.codex_recent_rows = payload.rows;
            ui.codex_recent_branch_cache = payload.branch_cache;
            ui.codex_recent_error = None;
            ui.codex_recent_selected_idx = 0;
            ui.codex_recent_selected_id =
                ui.codex_recent_rows.first().map(|r| r.session_id.clone());
            ui.codex_recent_table
                .select((!ui.codex_recent_rows.is_empty()).then_some(0));
            ui.toast = Some((
                i18n::format_recent_loaded(ui.language, ui.codex_recent_rows.len()),
                Instant::now(),
            ));
        }
        Err(err) => {
            ui.codex_recent_error = Some(err.clone());
            ui.toast = Some((
                i18n::format_recent_load_failed(ui.language, &err),
                Instant::now(),
            ));
        }
    }
    true
}

async fn load_codex_recent_rows(
    raw_cwd: bool,
    mut branch_cache: HashMap<String, Option<String>>,
) -> anyhow::Result<CodexRecentRefreshPayload> {
    let since = Duration::from_secs(24 * 60 * 60);
    let list = crate::sessions::find_recent_codex_sessions(since, 500).await?;
    let mut rows = Vec::with_capacity(list.len());
    let mut missing_roots = Vec::new();
    let mut missing_seen = HashSet::new();
    for s in list {
        let cwd_opt = s.cwd.clone();
        let cwd = cwd_opt.as_deref().unwrap_or("-");
        let root = if raw_cwd {
            cwd.to_string()
        } else {
            crate::sessions::infer_project_root_from_cwd(cwd)
        };
        let branch =
            if root.trim().is_empty() || root == "-" || !std::path::Path::new(&root).exists() {
                None
            } else if let Some(v) = branch_cache.get(&root) {
                v.clone()
            } else {
                if missing_seen.insert(root.clone()) {
                    missing_roots.push(root.clone());
                }
                None
            };
        rows.push(RecentCodexRow {
            root,
            branch,
            session_id: s.id,
            cwd: cwd_opt,
            mtime_ms: s.mtime_ms,
        });
    }

    let mut branch_stream = stream::iter(missing_roots)
        .map(|root| async move {
            let branch = read_git_branch_shallow(&root).await;
            (root, branch)
        })
        .buffer_unordered(CODEX_RECENT_BRANCH_LOOKUP_CONCURRENCY);
    while let Some((root, branch)) = branch_stream.next().await {
        branch_cache.insert(root, branch);
    }

    for row in &mut rows {
        if row.branch.is_none()
            && let Some(branch) = branch_cache.get(&row.root)
        {
            row.branch = branch.clone();
        }
    }

    Ok(CodexRecentRefreshPayload { rows, branch_cache })
}

async fn read_git_branch_shallow(workdir: &str) -> Option<String> {
    use tokio::fs;

    let root = std::path::PathBuf::from(workdir);
    if !root.is_absolute() {
        return None;
    }

    let dot_git = root.join(".git");
    if !dot_git.exists() {
        return None;
    }

    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        let content = fs::read_to_string(&dot_git).await.ok()?;
        let first = content.lines().next()?.trim();
        let path = first.strip_prefix("gitdir:")?.trim();
        let mut p = std::path::PathBuf::from(path);
        if p.is_relative() {
            p = root.join(p);
        }
        p
    };

    let head = fs::read_to_string(gitdir.join("HEAD")).await.ok()?;
    let head = head.lines().next().unwrap_or("").trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        return Some(r.rsplit('/').next().unwrap_or(r).to_string());
    }
    if head.len() >= 8 {
        Some(head[..8].to_string())
    } else if head.is_empty() {
        None
    } else {
        Some(head.to_string())
    }
}
