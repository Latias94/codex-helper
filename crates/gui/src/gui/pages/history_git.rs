use std::collections::HashMap;

use super::history_workdir_from_cwd;
use super::*;

fn find_git_root_upward(workdir: &str) -> Option<std::path::PathBuf> {
    let trimmed = workdir.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return None;
    }
    let path = std::path::PathBuf::from(trimmed);
    if !path.is_absolute() {
        return None;
    }
    if !path.exists() {
        return None;
    }

    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let mut cur = canonical.clone();
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

pub(super) fn read_git_branch_shallow(workdir: &str) -> Option<String> {
    let root = find_git_root_upward(workdir)?;
    let dot_git = root.join(".git");
    if !dot_git.exists() {
        return None;
    }

    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let first = content.lines().next()?.trim();
        let path = first.strip_prefix("gitdir:")?.trim();
        let mut p = std::path::PathBuf::from(path);
        if p.is_relative() {
            p = root.join(p);
        }
        p
    };

    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
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

pub(super) fn refresh_branch_cache_for_sessions(
    branch_by_workdir: &mut HashMap<String, Option<String>>,
    infer_git_root: bool,
    sessions: &[SessionSummary],
) {
    branch_by_workdir.clear();
    for session in sessions {
        let Some(cwd) = session.cwd.as_deref() else {
            continue;
        };
        let workdir = history_workdir_from_cwd(cwd, infer_git_root);
        if workdir == "-" || workdir.trim().is_empty() {
            continue;
        }
        if branch_by_workdir.contains_key(workdir.as_str()) {
            continue;
        }
        let branch = read_git_branch_shallow(workdir.as_str());
        branch_by_workdir.insert(workdir, branch);
    }
}
