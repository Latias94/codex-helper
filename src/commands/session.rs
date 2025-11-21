use crate::sessions::{
    SessionSummary, find_codex_sessions_for_current_dir, find_codex_sessions_for_dir,
    search_codex_sessions_for_current_dir, search_codex_sessions_for_dir,
};
use crate::{CliResult, SessionCommand};

pub async fn handle_session_cmd(cmd: SessionCommand) -> CliResult<()> {
    match cmd {
        SessionCommand::List { limit, path } => {
            let sessions: Vec<SessionSummary> = if let Some(p) = path {
                let root = std::path::PathBuf::from(p);
                find_codex_sessions_for_dir(&root, limit).await?
            } else {
                find_codex_sessions_for_current_dir(limit).await?
            };
            if sessions.is_empty() {
                println!("No Codex sessions found under ~/.codex/sessions");
            } else {
                println!("Recent Codex sessions (newest first):");
                for s in sessions {
                    let updated = s.updated_at.as_deref().unwrap_or("-");
                    let cwd = s.cwd.as_deref().unwrap_or("-");
                    let preview_raw = s
                        .first_user_message
                        .as_deref()
                        .unwrap_or("")
                        .replace('\n', " ");
                    let preview = super::doctor::truncate_for_display(&preview_raw, 80);

                    println!("- id: {}", s.id);
                    println!("  updated: {} | cwd: {}", updated, cwd);
                    if !preview.is_empty() {
                        println!("  prompt: {}", preview);
                    }
                    println!();
                }
            }
        }
        SessionCommand::Last { path } => {
            let mut sessions = if let Some(p) = path {
                let root = std::path::PathBuf::from(p);
                find_codex_sessions_for_dir(&root, 1).await?
            } else {
                find_codex_sessions_for_current_dir(1).await?
            };
            if let Some(s) = sessions.pop() {
                println!("Last Codex session for current project:");
                println!("  id: {}", s.id);
                println!("  updated_at: {}", s.updated_at.as_deref().unwrap_or("-"));
                println!("  cwd: {}", s.cwd.as_deref().unwrap_or("-"));
                if let Some(msg) = s.first_user_message.as_deref() {
                    let msg_single = msg.replace('\n', " ");
                    println!("  first_prompt: {}", msg_single);
                }
                println!();
                println!("Resume with:");
                println!("  codex resume {}", s.id);
            } else {
                println!("No Codex sessions found under ~/.codex/sessions");
            }
        }
        SessionCommand::Search { query, limit, path } => {
            let sessions: Vec<SessionSummary> = if let Some(p) = path {
                let root = std::path::PathBuf::from(p);
                search_codex_sessions_for_dir(&root, &query, limit).await?
            } else {
                search_codex_sessions_for_current_dir(&query, limit).await?
            };
            if sessions.is_empty() {
                println!(
                    "No Codex sessions under ~/.codex/sessions matched query: {}",
                    query
                );
            } else {
                println!("Sessions matching '{}':", query);
                for s in sessions {
                    let updated = s.updated_at.as_deref().unwrap_or("-");
                    let cwd = s.cwd.as_deref().unwrap_or("-");
                    let preview_raw = s
                        .first_user_message
                        .as_deref()
                        .unwrap_or("")
                        .replace('\n', " ");
                    let preview = super::doctor::truncate_for_display(&preview_raw, 80);

                    println!("- id: {}", s.id);
                    println!("  updated: {} | cwd: {}", updated, cwd);
                    if !preview.is_empty() {
                        println!("  prompt: {}", preview);
                    }
                    println!();
                }
            }
        }
        SessionCommand::Export { id, format, output } => {
            // For now, only lookup by scanning all sessions under current dir.
            let cwd = std::env::current_dir().expect("failed to resolve current directory");
            let sessions = find_codex_sessions_for_dir(&cwd, usize::MAX).await?;
            let Some(sess) = sessions.into_iter().find(|s| s.id == id) else {
                println!("Session with id {} not found under ~/.codex/sessions", id);
                return Ok(());
            };

            let fmt = format.to_lowercase();
            let content = if fmt == "json" {
                // Minimal JSON export: same fields as SessionSummary for now.
                let json = serde_json::json!({
                    "id": sess.id,
                    "cwd": sess.cwd,
                    "created_at": sess.created_at,
                    "updated_at": sess.updated_at,
                    "first_user_message": sess.first_user_message,
                    "path": sess.path,
                });
                serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".to_string())
            } else {
                // Default: markdown export with basic header and first prompt.
                let mut md = String::new();
                md.push_str("# Codex session\n\n");
                md.push_str(&format!("- id: `{}`\n", sess.id));
                if let Some(updated) = sess.updated_at.as_deref() {
                    md.push_str(&format!("- updated_at: `{}`\n", updated));
                }
                if let Some(cwd) = sess.cwd.as_deref() {
                    md.push_str(&format!("- cwd: `{}`\n", cwd));
                }
                md.push_str("\n");
                if let Some(msg) = sess.first_user_message.as_deref() {
                    md.push_str("## First user message\n\n");
                    md.push_str(msg);
                    md.push('\n');
                }
                md
            };

            if let Some(path) = output {
                let out_path = std::path::PathBuf::from(path);
                if let Some(parent) = out_path.parent() {
                    if !parent.as_os_str().is_empty() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            return Err(crate::CliError::Other(format!(
                                "failed to create parent dir {:?}: {}",
                                parent, e
                            )));
                        }
                    }
                }
                if let Err(e) = std::fs::write(&out_path, content) {
                    return Err(crate::CliError::Other(format!(
                        "failed to write export file {:?}: {}",
                        out_path, e
                    )));
                }
                println!("Exported session {} to {:?}", id, out_path);
            } else {
                println!("{content}");
            }
        }
    }

    Ok(())
}
