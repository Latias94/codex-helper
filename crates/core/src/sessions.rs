use std::cmp::{Ordering, Reverse};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use futures_util::stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};

use crate::config::codex_sessions_dir;
use crate::file_replace::write_bytes_file_async;

mod stats_cache;
mod transcript;

use stats_cache::{SessionStatsCache, SessionStatsSnapshot};
pub use transcript::{codex_session_transcript_tail_contains_query, read_codex_session_transcript};

/// Summary information for a Codex conversation session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionSummarySource {
    #[default]
    LocalFile,
    ObservedOnly,
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// RFC3339 timestamp string for the most recent assistant message, if available.
    pub last_response_at: Option<String>,
    /// Number of user turns (from `event_msg` user_message).
    pub user_turns: usize,
    /// Number of assistant messages (from `response_item` message role=assistant).
    pub assistant_turns: usize,
    /// Conversation rounds (best-effort; currently `min(user_turns, assistant_turns)`).
    pub rounds: usize,
    pub first_user_message: Option<String>,
    pub source: SessionSummarySource,
    pub sort_hint_ms: Option<u64>,
}

/// Basic metadata for a Codex session (best-effort parsed from JSONL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub cwd: Option<String>,
    pub created_at: Option<String>,
}

/// A single transcript message extracted from a Codex session JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTranscriptMessage {
    pub timestamp: Option<String>,
    pub role: String,
    pub text: String,
}

/// Minimal data for printing `project_root session_id` style lists.
#[derive(Debug, Clone)]
pub struct RecentSession {
    pub id: String,
    pub cwd: Option<String>,
    pub mtime_ms: u64,
}

#[cfg(feature = "gui")]
#[derive(Debug, Clone)]
pub struct SessionDayDir {
    pub date: String,
    pub path: PathBuf,
}

#[cfg(feature = "gui")]
#[derive(Debug, Clone)]
pub struct SessionIndexItem {
    pub id: String,
    pub path: PathBuf,
    pub cwd: Option<String>,
    pub created_at: Option<String>,
    pub updated_hint: Option<String>,
    pub mtime_ms: u64,
    pub first_user_message: Option<String>,
}

pub fn infer_project_root_from_cwd(cwd: &str) -> String {
    let path = std::path::PathBuf::from(cwd);
    if !path.is_absolute() {
        return cwd.to_string();
    }

    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let mut cur = canonical.clone();
    loop {
        if cur.join(".git").exists() {
            return cur.to_string_lossy().to_string();
        }
        if !cur.pop() {
            break;
        }
    }
    canonical.to_string_lossy().to_string()
}

const MAX_SCAN_FILES: usize = 10_000;
const HEAD_SCAN_LINES: usize = 512;
const IO_CHUNK_SIZE: usize = 64 * 1024;
const TAIL_SCAN_MAX_BYTES: usize = 1024 * 1024;
const SESSION_IO_CONCURRENCY: usize = 8;

const MAX_SCAN_FILES_RECENT: usize = 200_000;

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

    let mut matched: Vec<SessionHeader> = Vec::new();
    let mut others: Vec<SessionHeader> = Vec::new();
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

                    let header_opt = read_session_header(&path, root_dir).await?;
                    let Some(header) = header_opt else {
                        continue;
                    };

                    if header.is_cwd_match {
                        matched.push(header);
                    } else {
                        others.push(header);
                    }
                }
            }
        }
    }

    select_and_expand_headers(matched, others, limit).await
}

/// Search Codex sessions for user messages containing the given substring.
/// Matching is case-insensitive and only considers the first user message per session.
pub async fn search_codex_sessions_for_dir(
    root_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SessionSummary>> {
    let needle = query.to_lowercase();

    let root = codex_sessions_dir();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut matched: Vec<SessionHeader> = Vec::new();
    let mut others: Vec<SessionHeader> = Vec::new();
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

                    let header_opt = read_session_header(&path, root_dir).await?;
                    let Some(header) = header_opt else {
                        continue;
                    };
                    if !header
                        .first_user_message
                        .to_lowercase()
                        .contains(needle.as_str())
                    {
                        continue;
                    }

                    if header.is_cwd_match {
                        matched.push(header);
                    } else {
                        others.push(header);
                    }
                }
            }
        }
    }

    select_and_expand_headers(matched, others, limit).await
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

/// List recent Codex sessions across all projects, filtered by session file mtime.
///
/// This is optimized for "resume" workflows: it avoids counting turns/timestamps and only reads the
/// `session_meta` header for sessions that pass the recency filter.
pub async fn find_recent_codex_sessions(
    since: Duration,
    limit: usize,
) -> Result<Vec<RecentSession>> {
    let root = codex_sessions_dir();
    find_recent_codex_sessions_in_dir(&root, since, limit).await
}

#[cfg(feature = "gui")]
pub async fn find_recent_codex_session_summaries(
    since: Duration,
    limit: usize,
) -> Result<Vec<SessionSummary>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let sessions_dir = codex_sessions_dir();
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let since_ms = since.as_millis().min(u64::MAX as u128) as u64;
    let threshold_ms = now_ms.saturating_sub(since_ms);

    let mut headers: Vec<SessionHeader> = Vec::new();
    let mut scanned_files: usize = 0;

    let year_dirs = collect_dirs_desc(&sessions_dir, |s| s.parse::<u32>().ok()).await?;
    'outer: for (_year, year_path) in year_dirs {
        let month_dirs = collect_dirs_desc(&year_path, |s| s.parse::<u8>().ok()).await?;
        for (_month, month_path) in month_dirs {
            let day_dirs = collect_dirs_desc(&month_path, |s| s.parse::<u8>().ok()).await?;
            for (_day, day_path) in day_dirs {
                let day_files = collect_rollout_files_sorted(&day_path).await?;
                for path in day_files {
                    if scanned_files >= MAX_SCAN_FILES_RECENT {
                        break 'outer;
                    }
                    scanned_files += 1;

                    let meta = match fs::metadata(&path).await {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    let mtime_ms = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
                        .unwrap_or(0);
                    if mtime_ms < threshold_ms {
                        continue;
                    }

                    let header_opt = read_session_header(&path, &cwd).await?;
                    let Some(header) = header_opt else {
                        continue;
                    };
                    headers.push(header);
                }
            }
        }
    }

    select_and_expand_headers(Vec::new(), headers, limit).await
}

#[cfg(feature = "gui")]
pub async fn list_codex_session_day_dirs(limit: usize) -> Result<Vec<SessionDayDir>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let root = codex_sessions_dir();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut out: Vec<SessionDayDir> = Vec::new();
    let year_dirs = collect_dirs_desc(&root, |s| s.parse::<u32>().ok()).await?;
    'outer: for (year, year_path) in year_dirs {
        let month_dirs = collect_dirs_desc(&year_path, |s| s.parse::<u8>().ok()).await?;
        for (month, month_path) in month_dirs {
            let day_dirs = collect_dirs_desc(&month_path, |s| s.parse::<u8>().ok()).await?;
            for (day, day_path) in day_dirs {
                out.push(SessionDayDir {
                    date: format!("{year:04}-{month:02}-{day:02}"),
                    path: day_path,
                });
                if out.len() >= limit {
                    break 'outer;
                }
            }
        }
    }
    Ok(out)
}

#[cfg(feature = "gui")]
pub async fn list_codex_sessions_in_day_dir(
    day_dir: &Path,
    limit: usize,
) -> Result<Vec<SessionIndexItem>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    if !day_dir.exists() {
        return Ok(Vec::new());
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let day_files = collect_rollout_files_sorted(day_dir).await?;
    let mut out: Vec<SessionIndexItem> = Vec::new();
    for chunk in day_files.chunks(SESSION_IO_CONCURRENCY) {
        let cwd = cwd.clone();
        let mut stream = stream::iter(chunk.iter().cloned())
            .map(move |path| {
                let cwd = cwd.clone();
                async move { read_session_index_item(path, cwd).await }
            })
            .buffer_unordered(SESSION_IO_CONCURRENCY);

        while let Some(item) = stream.next().await {
            if let Some(item) = item? {
                out.push(item);
            }
        }
        if out.len() >= limit {
            break;
        }
    }

    out.sort_by_key(|item| Reverse(item.mtime_ms));
    out.truncate(limit);
    Ok(out)
}

#[cfg(feature = "gui")]
async fn read_session_index_item(path: PathBuf, cwd: PathBuf) -> Result<Option<SessionIndexItem>> {
    let header_opt = read_session_header(&path, &cwd).await?;
    let Some(mut header) = header_opt else {
        return Ok(None);
    };
    header.updated_hint = read_last_timestamp_from_tail(&header.path)
        .await?
        .or_else(|| header.created_at.clone());
    Ok(Some(SessionIndexItem {
        id: header.id,
        path: header.path,
        cwd: header.cwd,
        created_at: header.created_at,
        updated_hint: header.updated_hint,
        mtime_ms: header.mtime_ms,
        first_user_message: Some(header.first_user_message),
    }))
}

async fn find_recent_codex_sessions_in_dir(
    sessions_dir: &Path,
    since: Duration,
    limit: usize,
) -> Result<Vec<RecentSession>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    if since.is_zero() {
        return Ok(Vec::new());
    }
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let since_ms = since.as_millis().min(u64::MAX as u128) as u64;
    let threshold_ms = now_ms.saturating_sub(since_ms);

    let mut out: Vec<RecentSession> = Vec::new();
    let mut scanned_files: usize = 0;

    let year_dirs = collect_dirs_desc(sessions_dir, |s| s.parse::<u32>().ok()).await?;
    'outer: for (_year, year_path) in year_dirs {
        let month_dirs = collect_dirs_desc(&year_path, |s| s.parse::<u8>().ok()).await?;
        for (_month, month_path) in month_dirs {
            let day_dirs = collect_dirs_desc(&month_path, |s| s.parse::<u8>().ok()).await?;
            for (_day, day_path) in day_dirs {
                let day_files = collect_rollout_files_sorted(&day_path).await?;
                for path in day_files {
                    if scanned_files >= MAX_SCAN_FILES_RECENT {
                        break 'outer;
                    }
                    scanned_files += 1;

                    let meta = match fs::metadata(&path).await {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    let mtime_ms = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
                        .unwrap_or(0);
                    if mtime_ms < threshold_ms {
                        continue;
                    }

                    let file_id = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .and_then(parse_timestamp_and_uuid)
                        .map(|(_, uuid)| uuid);

                    let meta = read_codex_session_meta(&path).await?;
                    let (id, cwd) = if let Some(meta) = meta {
                        (meta.id, meta.cwd)
                    } else if let Some(id) = file_id {
                        (id, None)
                    } else {
                        continue;
                    };

                    out.push(RecentSession { id, cwd, mtime_ms });
                }
            }
        }
    }

    out.sort_by_key(|item| Reverse((item.mtime_ms, item.id.clone())));
    out.truncate(limit);
    Ok(out)
}

/// Find a Codex session's cwd by its session id (UUID suffix in rollout filename).
///
/// This is best-effort and scans session files from newest to oldest until it finds a match.
pub async fn find_codex_session_cwd_by_id(session_id: &str) -> Result<Option<String>> {
    let root = codex_sessions_dir();
    if !root.exists() {
        return Ok(None);
    }

    let year_dirs = collect_dirs_desc(&root, |s| s.parse::<u32>().ok()).await?;
    for (_year, year_path) in year_dirs {
        let month_dirs = collect_dirs_desc(&year_path, |s| s.parse::<u8>().ok()).await?;
        for (_month, month_path) in month_dirs {
            let day_dirs = collect_dirs_desc(&month_path, |s| s.parse::<u8>().ok()).await?;
            for (_day, day_path) in day_dirs {
                let day_files = collect_rollout_files_sorted(&day_path).await?;
                for path in day_files {
                    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let Some((_ts, uuid)) = parse_timestamp_and_uuid(name) else {
                        continue;
                    };
                    if uuid != session_id {
                        continue;
                    }

                    let file = fs::File::open(&path)
                        .await
                        .with_context(|| format!("failed to open session file {:?}", path))?;
                    let reader = BufReader::new(file);
                    let mut lines = reader.lines();
                    while let Some(line) = lines.next_line().await? {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let value: Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        if let Some(meta) = parse_session_meta(&value) {
                            return Ok(meta.cwd);
                        }
                    }

                    return Ok(None);
                }
            }
        }
    }

    Ok(None)
}

/// Best-effort: locate a Codex session JSONL file by session id.
///
/// We first try to match the UUID suffix in the `rollout-...-<uuid>.jsonl` filename (fast path),
/// then fall back to scanning session_meta records to match `payload.id`.
pub async fn find_codex_session_file_by_id(session_id: &str) -> Result<Option<PathBuf>> {
    Ok(find_codex_session_files_by_ids(&[session_id.to_string()])
        .await?
        .remove(session_id))
}

pub async fn find_codex_session_files_by_ids(
    session_ids: &[String],
) -> Result<HashMap<String, PathBuf>> {
    find_codex_session_files_by_ids_in_dir(&codex_sessions_dir(), session_ids).await
}

async fn find_codex_session_files_by_ids_in_dir(
    root: &Path,
    session_ids: &[String],
) -> Result<HashMap<String, PathBuf>> {
    if !root.exists() || session_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut remaining = session_ids
        .iter()
        .map(|sid| sid.trim())
        .filter(|sid| !sid.is_empty())
        .map(ToOwned::to_owned)
        .collect::<std::collections::HashSet<_>>();
    if remaining.is_empty() {
        return Ok(HashMap::new());
    }

    let mut found = HashMap::new();
    let mut scanned_files: usize = 0;
    let year_dirs = collect_dirs_desc(root, |s| s.parse::<u32>().ok()).await?;

    'outer: for (_year, year_path) in year_dirs {
        let month_dirs = collect_dirs_desc(&year_path, |s| s.parse::<u8>().ok()).await?;
        for (_month, month_path) in month_dirs {
            let day_dirs = collect_dirs_desc(&month_path, |s| s.parse::<u8>().ok()).await?;
            for (_day, day_path) in day_dirs {
                let day_files = collect_rollout_files_sorted(&day_path).await?;
                for path in day_files {
                    if scanned_files >= MAX_SCAN_FILES || remaining.is_empty() {
                        break 'outer;
                    }
                    scanned_files += 1;

                    if let Some(name) = path.file_name().and_then(|s| s.to_str())
                        && let Some((_ts, uuid)) = parse_timestamp_and_uuid(name)
                        && remaining.remove(&uuid)
                    {
                        found.insert(uuid.to_string(), path.clone());
                        if remaining.is_empty() {
                            break 'outer;
                        }
                        continue;
                    }

                    if let Some(meta) = read_codex_session_meta(&path).await?
                        && remaining.remove(meta.id.as_str())
                    {
                        found.insert(meta.id, path);
                        if remaining.is_empty() {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }

    Ok(found)
}

/// Read the `session_meta` record from a Codex session JSONL file (best-effort).
pub async fn read_codex_session_meta(path: &Path) -> Result<Option<SessionMeta>> {
    let file = fs::File::open(path)
        .await
        .with_context(|| format!("failed to open session file {:?}", path))?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    let mut lines_scanned = 0usize;
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines_scanned += 1;
        if lines_scanned > HEAD_SCAN_LINES {
            break;
        }

        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(meta) = parse_session_meta(&value) {
            return Ok(Some(SessionMeta {
                id: meta.id,
                cwd: meta.cwd,
                created_at: meta.created_at,
            }));
        }
    }

    Ok(None)
}

#[cfg(test)]
async fn summarize_session_for_current_dir(
    path: &Path,
    cwd: &Path,
) -> Result<Option<SessionSummary>> {
    let header_opt = read_session_header(path, cwd).await?;
    let Some(header) = header_opt else {
        return Ok(None);
    };
    Ok(Some(expand_header_to_summary_uncached(header).await?))
}

struct SessionMetaInfo {
    id: String,
    cwd: Option<String>,
    created_at: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionHeader {
    id: String,
    path: PathBuf,
    cwd: Option<String>,
    created_at: Option<String>,
    /// File modified time in milliseconds since epoch (used for cheap recency sorting).
    mtime_ms: u64,
    /// Best-effort: timestamp of the most recent JSONL record (from the file tail; only computed for displayed rows).
    updated_hint: Option<String>,
    first_user_message: String,
    is_cwd_match: bool,
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

fn user_message_text(value: &Value) -> Option<&str> {
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
    payload.get("message").and_then(|v| v.as_str())
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

async fn read_session_header(path: &Path, cwd: &Path) -> Result<Option<SessionHeader>> {
    let meta = fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat session file {:?}", path))?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let file = fs::File::open(path)
        .await
        .with_context(|| format!("failed to open session file {:?}", path))?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    let mut session_id: Option<String> = None;
    let mut cwd_str: Option<String> = None;
    let mut created_at: Option<String> = None;
    let mut first_user_message: Option<String> = None;

    let mut lines_scanned = 0usize;
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines_scanned += 1;
        if lines_scanned > HEAD_SCAN_LINES {
            break;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if session_id.is_none()
            && let Some(meta) = parse_session_meta(&value)
        {
            session_id = Some(meta.id);
            cwd_str = meta.cwd;
            created_at = meta.created_at;
        }

        if first_user_message.is_none()
            && let Some(msg) = user_message_text(&value)
        {
            first_user_message = Some(msg.to_string());
        }

        if session_id.is_some() && first_user_message.is_some() {
            break;
        }
    }

    let Some(id) = session_id else {
        return Ok(None);
    };
    let Some(first_user_message) = first_user_message else {
        return Ok(None);
    };

    let cwd_value = cwd_str.clone();
    let is_cwd_match = cwd_value
        .as_deref()
        .map(|s| path_matches_current_dir(s, cwd))
        .unwrap_or(false);

    Ok(Some(SessionHeader {
        id,
        path: path.to_path_buf(),
        cwd: cwd_value,
        created_at,
        mtime_ms,
        updated_hint: None,
        first_user_message,
        is_cwd_match,
    }))
}

async fn select_and_expand_headers(
    matched: Vec<SessionHeader>,
    others: Vec<SessionHeader>,
    limit: usize,
) -> Result<Vec<SessionSummary>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut chosen = if !matched.is_empty() { matched } else { others };
    // Use file mtime for cheap recency ordering; this correctly surfaces sessions that were resumed
    // (older filename timestamp but recently appended to).
    chosen.sort_by_key(|item| Reverse(item.mtime_ms));
    if chosen.len() > limit {
        chosen.truncate(limit);
    }

    let cache = Arc::new(Mutex::new(SessionStatsCache::load_default().await));
    let mut out: Vec<SessionSummary> = Vec::with_capacity(chosen.len().min(limit));
    let mut stream = stream::iter(chosen)
        .map(|header| {
            let cache = Arc::clone(&cache);
            async move { expand_header_to_summary_cached(cache, header).await }
        })
        .buffer_unordered(SESSION_IO_CONCURRENCY);

    while let Some(summary) = stream.next().await {
        out.push(summary?);
    }

    drop(stream);
    let mut cache = Arc::try_unwrap(cache)
        .map_err(|_| anyhow!("session stats cache still has active workers"))?
        .into_inner()
        .map_err(|_| anyhow!("session stats cache lock poisoned"))?;
    cache.save_if_dirty().await?;

    sort_by_updated_desc(&mut out);
    out.truncate(limit);
    Ok(out)
}

fn build_summary_from_stats(
    header: SessionHeader,
    user_turns: usize,
    assistant_turns: usize,
    last_response_at: Option<String>,
) -> SessionSummary {
    let rounds = user_turns.min(assistant_turns);
    let updated_at = last_response_at
        .clone()
        .or_else(|| header.updated_hint.clone())
        .or_else(|| header.created_at.clone());

    SessionSummary {
        id: header.id,
        path: header.path,
        cwd: header.cwd,
        created_at: header.created_at,
        updated_at,
        last_response_at,
        user_turns,
        assistant_turns,
        rounds,
        first_user_message: Some(header.first_user_message),
        source: SessionSummarySource::LocalFile,
        sort_hint_ms: None,
    }
}

async fn expand_header_to_summary_cached(
    cache: Arc<Mutex<SessionStatsCache>>,
    mut header: SessionHeader,
) -> Result<SessionSummary> {
    let path = header.path.clone();
    let key = path.to_string_lossy().to_string();
    let meta = fs::metadata(&path)
        .await
        .with_context(|| format!("failed to stat session file {:?}", path))?;
    let size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let cached = {
        let cache = cache
            .lock()
            .map_err(|_| anyhow!("session stats cache lock poisoned"))?;
        cache.lookup(&key, mtime_ms, size)
    };

    let stats = if let Some(stats) = cached {
        if stats.last_response_at.is_none() && header.updated_hint.is_none() {
            header.updated_hint = read_last_timestamp_from_tail(&path)
                .await?
                .or_else(|| header.created_at.clone());
        }
        stats
    } else {
        let (counts, tail) = tokio::join!(
            count_turns_in_file(&path),
            read_tail_timestamps(&path, true)
        );
        let (user_turns, assistant_turns) = counts?;
        let tail = tail?;
        header.updated_hint = tail.last_record_at.or_else(|| header.created_at.clone());

        let stats = SessionStatsSnapshot {
            user_turns,
            assistant_turns,
            last_response_at: tail.last_assistant_at,
        };
        {
            let mut cache = cache
                .lock()
                .map_err(|_| anyhow!("session stats cache lock poisoned"))?;
            cache.insert(key, mtime_ms, size, &stats);
        }
        stats
    };

    Ok(build_summary_from_stats(
        header,
        stats.user_turns,
        stats.assistant_turns,
        stats.last_response_at,
    ))
}

#[cfg(test)]
async fn expand_header_to_summary_uncached(header: SessionHeader) -> Result<SessionSummary> {
    let (user_turns, assistant_turns) = count_turns_in_file(&header.path).await?;
    let last_response_at = read_last_assistant_timestamp_from_tail(&header.path).await?;
    Ok(build_summary_from_stats(
        header,
        user_turns,
        assistant_turns,
        last_response_at,
    ))
}

async fn count_turns_in_file(path: &Path) -> Result<(usize, usize)> {
    const USER_TURN_NEEDLE: &[u8] = br#""payload":{"type":"user_message""#;
    const ASSISTANT_TURN_NEEDLE: &[u8] = br#""role":"assistant""#;

    let mut file = fs::File::open(path)
        .await
        .with_context(|| format!("failed to open session file {:?}", path))?;

    let mut buf = vec![0u8; IO_CHUNK_SIZE];
    let mut user_carry: Vec<u8> = Vec::new();
    let mut assistant_carry: Vec<u8> = Vec::new();
    let mut user_total = 0usize;
    let mut assistant_total = 0usize;
    let mut user_window: Vec<u8> = Vec::with_capacity(IO_CHUNK_SIZE + USER_TURN_NEEDLE.len());
    let mut assistant_window: Vec<u8> =
        Vec::with_capacity(IO_CHUNK_SIZE + ASSISTANT_TURN_NEEDLE.len());

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }

        user_window.clear();
        user_window.extend_from_slice(&user_carry);
        user_window.extend_from_slice(&buf[..n]);
        user_total = user_total.saturating_add(count_subslice(&user_window, USER_TURN_NEEDLE));

        assistant_window.clear();
        assistant_window.extend_from_slice(&assistant_carry);
        assistant_window.extend_from_slice(&buf[..n]);
        assistant_total = assistant_total
            .saturating_add(count_subslice(&assistant_window, ASSISTANT_TURN_NEEDLE));

        let user_keep = USER_TURN_NEEDLE.len().saturating_sub(1);
        user_carry = if user_keep > 0 && user_window.len() >= user_keep {
            user_window[user_window.len() - user_keep..].to_vec()
        } else {
            Vec::new()
        };

        let assistant_keep = ASSISTANT_TURN_NEEDLE.len().saturating_sub(1);
        assistant_carry = if assistant_keep > 0 && assistant_window.len() >= assistant_keep {
            assistant_window[assistant_window.len() - assistant_keep..].to_vec()
        } else {
            Vec::new()
        };
    }

    Ok((user_total, assistant_total))
}

fn count_subslice(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    if haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

#[derive(Debug, Default)]
struct TailTimestamps {
    last_record_at: Option<String>,
    last_assistant_at: Option<String>,
}

async fn read_last_timestamp_from_tail(path: &Path) -> Result<Option<String>> {
    Ok(read_tail_timestamps(path, false).await?.last_record_at)
}

#[cfg(test)]
async fn read_last_assistant_timestamp_from_tail(path: &Path) -> Result<Option<String>> {
    Ok(read_tail_timestamps(path, true).await?.last_assistant_at)
}

async fn read_tail_timestamps(path: &Path, include_assistant: bool) -> Result<TailTimestamps> {
    const ASSISTANT_ROLE_NEEDLE: &[u8] = br#""role":"assistant""#;

    let mut file = fs::File::open(path)
        .await
        .with_context(|| format!("failed to open session file {:?}", path))?;
    let meta = file
        .metadata()
        .await
        .with_context(|| format!("failed to stat session file {:?}", path))?;
    let mut pos = meta.len();
    if pos == 0 {
        return Ok(TailTimestamps::default());
    }

    let mut scanned = 0usize;
    let mut carry: Vec<u8> = Vec::new();
    let chunk_size = IO_CHUNK_SIZE as u64;
    let mut found = TailTimestamps::default();

    while pos > 0 && scanned < TAIL_SCAN_MAX_BYTES {
        let start = pos.saturating_sub(chunk_size);
        let size = (pos - start) as usize;
        file.seek(std::io::SeekFrom::Start(start)).await?;

        let mut chunk = vec![0u8; size];
        file.read_exact(&mut chunk).await?;
        scanned = scanned.saturating_add(size);

        if !carry.is_empty() {
            chunk.extend_from_slice(&carry);
        }

        // Iterate lines from the end.
        let mut end = chunk.len();
        while end > 0 {
            let mut begin = end;
            while begin > 0 && chunk[begin - 1] != b'\n' {
                begin -= 1;
            }
            let line = chunk[begin..end].trim_ascii();
            end = begin.saturating_sub(1);

            if line.is_empty() {
                continue;
            }

            let wants_record = found.last_record_at.is_none();
            let wants_assistant = include_assistant
                && found.last_assistant_at.is_none()
                && contains_bytes(line, ASSISTANT_ROLE_NEEDLE);
            if !wants_record && !wants_assistant {
                continue;
            }

            let value: Value = match serde_json::from_slice(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str()) {
                let ts = ts.to_string();
                if wants_record {
                    found.last_record_at = Some(ts.clone());
                }
                if wants_assistant {
                    found.last_assistant_at = Some(ts);
                }
                if found.last_record_at.is_some()
                    && (!include_assistant || found.last_assistant_at.is_some())
                {
                    return Ok(found);
                }
            }
        }

        // Keep the partial first line for the next iteration.
        if let Some(first_nl) = chunk.iter().position(|b| *b == b'\n') {
            carry = chunk[..first_nl].to_vec();
        } else {
            carry = chunk;
        }

        pos = start;
    }

    Ok(found)
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

    // Timestamp format is stable and has a fixed width: "YYYY-MM-DDThh-mm-ss" (19 chars).
    const TS_LEN: usize = 19;
    if core.len() <= TS_LEN + 1 {
        return None;
    }
    let (ts, rest) = core.split_at(TS_LEN);
    let uuid = rest.strip_prefix('-')?;
    if uuid.is_empty() {
        return None;
    }
    Some((ts.to_string(), uuid.to_string()))
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
mod tests;
