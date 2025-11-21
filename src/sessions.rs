use std::cmp::{Ordering, Reverse};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::config::codex_sessions_dir;

/// Summary information for a Codex conversation session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub first_user_message: Option<String>,
    pub is_cwd_match: bool,
}

const MAX_SCAN_FILES: usize = 10_000;

/// Find recent Codex sessions for a given directory, preferring sessions whose cwd matches that directory
/// (or one of its ancestors/descendants). Results are ordered newest-first by updated_at.
pub async fn find_codex_sessions_for_dir(
    root_dir: &Path,
    limit: usize,
) -> Result<Vec<SessionSummary>> {
    let root = codex_sessions_dir();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut matched: Vec<SessionSummary> = Vec::new();
    let mut others: Vec<SessionSummary> = Vec::new();
    let mut scanned_files: usize = 0;

    let year_dirs = collect_dirs_desc(&root, |s| s.parse::<u32>().ok()).await?;

    'outer: for (_year, year_path) in year_dirs {
        let month_dirs = collect_dirs_desc(&year_path, |s| s.parse::<u8>().ok()).await?;
        for (_month, month_path) in month_dirs {
            let day_dirs = collect_dirs_desc(&month_path, |s| s.parse::<u8>().ok()).await?;
            for (_day, day_path) in day_dirs {
                let day_files = collect_rollout_files_sorted(&day_path).await?;
                for path in day_files {
                    if scanned_files >= MAX_SCAN_FILES {
                        break 'outer;
                    }
                    scanned_files += 1;

                    let summary_opt = summarize_session_for_current_dir(&path, root_dir).await?;
                    let Some(summary) = summary_opt else {
                        continue;
                    };

                    // Skip sessions with no user messages (e.g. Codex started but no prompt was sent).
                    if summary.first_user_message.is_none() {
                        continue;
                    }

                    if summary.is_cwd_match {
                        matched.push(summary);
                    } else {
                        others.push(summary);
                    }

                    if matched.len() >= limit && others.len() >= limit {
                        // We already have enough candidates from both sets; allow early exit.
                        if scanned_files >= MAX_SCAN_FILES {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }

    if !matched.is_empty() {
        sort_by_updated_desc(&mut matched);
        matched.truncate(limit);
        Ok(matched)
    } else {
        sort_by_updated_desc(&mut others);
        others.truncate(limit);
        Ok(others)
    }
}

/// Search Codex sessions for user messages containing the given substring.
/// Matching is case-insensitive and only considers the first user message per session.
pub async fn search_codex_sessions_for_dir(
    root_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SessionSummary>> {
    let mut sessions = find_codex_sessions_for_dir(root_dir, usize::MAX).await?;
    let needle = query.to_lowercase();
    sessions.retain(|s| {
        s.first_user_message
            .as_deref()
            .map(|m| m.to_lowercase().contains(&needle))
            .unwrap_or(false)
    });
    sort_by_updated_desc(&mut sessions);
    if sessions.len() > limit {
        sessions.truncate(limit);
    }
    Ok(sessions)
}

/// Convenience wrapper that uses the current working directory as the root for session matching.
pub async fn find_codex_sessions_for_current_dir(limit: usize) -> Result<Vec<SessionSummary>> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    find_codex_sessions_for_dir(&cwd, limit).await
}

/// Convenience wrapper to search sessions under the current working directory.
pub async fn search_codex_sessions_for_current_dir(
    query: &str,
    limit: usize,
) -> Result<Vec<SessionSummary>> {
    let cwd = std::env::current_dir().context("failed to resolve current directory")?;
    search_codex_sessions_for_dir(&cwd, query, limit).await
}

async fn summarize_session_for_current_dir(
    path: &Path,
    cwd: &Path,
) -> Result<Option<SessionSummary>> {
    let file = fs::File::open(path)
        .await
        .with_context(|| format!("failed to open session file {:?}", path))?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    let mut session_id: Option<String> = None;
    let mut cwd_str: Option<String> = None;
    let mut created_at: Option<String> = None;
    let mut first_user_message: Option<String> = None;
    let mut last_timestamp: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if last_timestamp.is_none()
            && let Some(ts) = value.get("timestamp").and_then(|v| v.as_str())
        {
            last_timestamp = Some(ts.to_string());
        }

        if session_id.is_none()
            && let Some(meta) = parse_session_meta(&value)
        {
            session_id = Some(meta.id);
            cwd_str = meta.cwd;
            created_at = meta.created_at;
        }

        if first_user_message.is_none()
            && let Some(msg) = parse_user_message(&value)
        {
            first_user_message = Some(msg);
        }
    }

    let id = match session_id {
        Some(id) => id,
        None => return Ok(None),
    };

    let cwd_value = cwd_str.clone();
    let is_cwd_match = cwd_value
        .as_deref()
        .map(|s| path_matches_current_dir(s, cwd))
        .unwrap_or(false);

    let updated_at = last_timestamp.or_else(|| created_at.clone());

    Ok(Some(SessionSummary {
        id,
        path: path.to_path_buf(),
        cwd: cwd_value,
        created_at,
        updated_at,
        first_user_message,
        is_cwd_match,
    }))
}

struct SessionMetaInfo {
    id: String,
    cwd: Option<String>,
    created_at: Option<String>,
}

fn parse_session_meta(value: &Value) -> Option<SessionMetaInfo> {
    let obj = value.as_object()?;
    let type_str = obj.get("type")?.as_str()?;
    if type_str != "session_meta" {
        return None;
    }

    let payload = obj.get("payload")?.as_object()?;
    let id = payload.get("id").and_then(|v| v.as_str())?.to_string();
    let cwd = payload
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let created_at = payload
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            obj.get("timestamp")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    Some(SessionMetaInfo {
        id,
        cwd,
        created_at,
    })
}

fn parse_user_message(value: &Value) -> Option<String> {
    let obj = value.as_object()?;
    let type_str = obj.get("type")?.as_str()?;
    if type_str != "event_msg" {
        return None;
    }
    let payload = obj.get("payload")?.as_object()?;
    let payload_type = payload.get("type")?.as_str()?;
    if payload_type != "user_message" {
        return None;
    }
    let msg = payload.get("message").and_then(|v| v.as_str())?;
    Some(msg.to_string())
}

fn path_matches_current_dir(session_cwd: &str, current_dir: &Path) -> bool {
    let session_path = PathBuf::from(session_cwd);
    if !session_path.is_absolute() {
        return false;
    }

    let current = std::fs::canonicalize(current_dir).unwrap_or_else(|_| current_dir.to_path_buf());
    let cwd = std::fs::canonicalize(&session_path).unwrap_or(session_path);

    current == cwd || current.starts_with(&cwd) || cwd.starts_with(&current)
}

async fn collect_dirs_desc<T, F>(parent: &Path, parse: F) -> std::io::Result<Vec<(T, PathBuf)>>
where
    T: Ord + Copy,
    F: Fn(&str) -> Option<T>,
{
    let mut dir = fs::read_dir(parent).await?;
    let mut vec: Vec<(T, PathBuf)> = Vec::new();
    while let Some(entry) = dir.next_entry().await? {
        if entry
            .file_type()
            .await
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
            && let Some(s) = entry.file_name().to_str()
            && let Some(v) = parse(s)
        {
            vec.push((v, entry.path()));
        }
    }
    vec.sort_by_key(|(v, _)| Reverse(*v));
    Ok(vec)
}

async fn collect_rollout_files_sorted(parent: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dir = fs::read_dir(parent).await?;
    let mut records: Vec<(String, String, PathBuf)> = Vec::new();

    while let Some(entry) = dir.next_entry().await? {
        if entry
            .file_type()
            .await
            .map(|ft| ft.is_file())
            .unwrap_or(false)
        {
            let name_os = entry.file_name();
            let Some(name) = name_os.to_str() else {
                continue;
            };
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            if let Some((ts, uuid)) = parse_timestamp_and_uuid(name) {
                records.push((ts, uuid, entry.path()));
            }
        }
    }

    records.sort_by(|a, b| {
        // Sort by timestamp desc, then UUID desc.
        match b.0.cmp(&a.0) {
            Ordering::Equal => b.1.cmp(&a.1),
            other => other,
        }
    });

    Ok(records.into_iter().map(|(_, _, path)| path).collect())
}

fn parse_timestamp_and_uuid(name: &str) -> Option<(String, String)> {
    // Expected: rollout-YYYY-MM-DDThh-mm-ss-<uuid>.jsonl
    let core = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;

    // Scan from the right for a '-' such that the suffix is a non-empty UUID string.
    let (sep_idx, uuid_str) = core.match_indices('-').rev().find_map(|(i, _)| {
        let candidate = &core[i + 1..];
        if !candidate.is_empty() {
            Some((i, candidate.to_string()))
        } else {
            None
        }
    })?;

    let ts_str = &core[..sep_idx];
    Some((ts_str.to_string(), uuid_str))
}

fn sort_by_updated_desc(vec: &mut [SessionSummary]) {
    vec.sort_by(|a, b| {
        let ta = a.updated_at.as_deref();
        let tb = b.updated_at.as_deref();
        match (ta, tb) {
            (Some(ta), Some(tb)) => tb.cmp(ta),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cwd_parent_of_current_dir_matches() {
        let base = std::env::current_dir().expect("cwd");
        let project = base.join("codex_project_parent");
        let child = project.join("subdir");
        let session_cwd = project.to_str().expect("project path utf8").to_string();

        assert!(
            path_matches_current_dir(&session_cwd, &child),
            "session cwd should match when it is a parent of current dir"
        );
    }

    #[test]
    fn session_cwd_child_of_current_dir_matches() {
        let base = std::env::current_dir().expect("cwd");
        let project = base.join("codex_project_child");
        let child = project.join("subdir");
        let session_cwd = child.to_str().expect("child path utf8").to_string();

        assert!(
            path_matches_current_dir(&session_cwd, &project),
            "session cwd should match when it is a child of current dir"
        );
    }

    #[test]
    fn unrelated_paths_do_not_match() {
        let base = std::env::current_dir().expect("cwd");
        let project = base.join("codex_project_main");
        let other = base.join("other_project_main");
        let session_cwd = other.to_str().expect("other path utf8").to_string();

        assert!(
            !path_matches_current_dir(&session_cwd, &project),
            "unrelated paths should not match"
        );
    }
}
