use crate::sessions::{
    SessionSummary, find_codex_session_file_by_id, find_codex_sessions_for_current_dir,
    find_codex_sessions_for_dir, find_recent_codex_sessions, read_codex_session_meta,
    read_codex_session_transcript, search_codex_sessions_for_current_dir,
    search_codex_sessions_for_dir,
};
use crate::{CliResult, RecentFormat, RecentTerminal, SessionCommand};

fn basename_lower(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_lowercase()
}

fn render_resume_cmd(template: &str, session_id: &str) -> String {
    template.replace("{id}", session_id)
}

fn infer_project_root_from_cwd(cwd: &str) -> String {
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

fn print_recent_sessions(
    format: RecentFormat,
    rows: &[(String, String, Option<String>, u64)],
) -> CliResult<()> {
    match format {
        RecentFormat::Text => {
            for (root, id, _cwd, _mtime_ms) in rows {
                println!("{root} {id}");
            }
        }
        RecentFormat::Tsv => {
            for (root, id, cwd, mtime_ms) in rows {
                let cwd = cwd.as_deref().unwrap_or("");
                println!("{root}\t{id}\t{cwd}\t{mtime_ms}");
            }
        }
        RecentFormat::Json => {
            #[derive(serde::Serialize)]
            struct Row<'a> {
                project_root: &'a str,
                session_id: &'a str,
                cwd: Option<&'a str>,
                mtime_ms: u64,
            }
            let json_rows: Vec<Row<'_>> = rows
                .iter()
                .map(|(root, id, cwd, mtime_ms)| Row {
                    project_root: root.as_str(),
                    session_id: id.as_str(),
                    cwd: cwd.as_deref(),
                    mtime_ms: *mtime_ms,
                })
                .collect();
            let s = serde_json::to_string_pretty(&json_rows).unwrap_or_else(|_| "[]".to_string());
            println!("{s}");
        }
    }
    Ok(())
}

fn spawn_cmd_dry_run(label: &str, program: &str, args: &[String]) {
    let joined = std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("DRY-RUN[{label}]: {joined}");
}

fn spawn_windows_terminal_wt(
    wt_window: i32,
    workdir: &str,
    shell: &str,
    keep_open: bool,
    command: &str,
    dry_run: bool,
) -> CliResult<()> {
    let shell_base = basename_lower(shell);
    let mut args: Vec<String> = Vec::new();
    args.push("-w".to_string());
    args.push(wt_window.to_string());
    args.push("new-tab".to_string());
    args.push("-d".to_string());
    args.push(workdir.to_string());
    args.push(shell.to_string());

    if shell_base.contains("pwsh") || shell_base.contains("powershell") {
        args.push("-ExecutionPolicy".to_string());
        args.push("Bypass".to_string());
        if keep_open {
            args.push("-NoExit".to_string());
        }
        args.push("-Command".to_string());
        args.push(command.to_string());
    } else if shell_base == "cmd" || shell_base == "cmd.exe" {
        args.push(if keep_open { "/k" } else { "/c" }.to_string());
        args.push(command.to_string());
    } else {
        return Err(crate::CliError::Other(format!(
            "unsupported shell for wt: {} (supported: pwsh/powershell/cmd)",
            shell
        )));
    }

    if dry_run {
        spawn_cmd_dry_run("wt", "wt", &args);
        return Ok(());
    }

    std::process::Command::new("wt")
        .args(&args)
        .spawn()
        .map_err(|e| {
            crate::CliError::Other(format!(
                "failed to spawn wt; is Windows Terminal installed and `wt` in PATH? ({e})"
            ))
        })?;
    Ok(())
}

fn spawn_wezterm(
    workdir: &str,
    shell: &str,
    keep_open: bool,
    command: &str,
    dry_run: bool,
) -> CliResult<()> {
    let shell_base = basename_lower(shell);
    let mut args: Vec<String> = Vec::new();
    args.push("start".to_string());
    args.push("--cwd".to_string());
    args.push(workdir.to_string());
    args.push("--".to_string());
    args.push(shell.to_string());

    if shell_base.contains("pwsh") || shell_base.contains("powershell") {
        if keep_open {
            args.push("-NoExit".to_string());
        }
        args.push("-Command".to_string());
        args.push(command.to_string());
    } else if shell_base == "sh" || shell_base == "bash" || shell_base == "zsh" {
        args.push("-lc".to_string());
        if keep_open {
            args.push(format!("{command}; exec {shell_base}"));
        } else {
            args.push(command.to_string());
        }
    } else {
        return Err(crate::CliError::Other(format!(
            "unsupported shell for wezterm: {} (supported: pwsh/powershell/sh/bash/zsh)",
            shell
        )));
    }

    if dry_run {
        spawn_cmd_dry_run("wezterm", "wezterm", &args);
        return Ok(());
    }

    std::process::Command::new("wezterm")
        .args(&args)
        .spawn()
        .map_err(|e| {
            crate::CliError::Other(format!(
                "failed to spawn wezterm; is WezTerm installed and `wezterm` in PATH? ({e})"
            ))
        })?;
    Ok(())
}

pub async fn handle_session_cmd(cmd: SessionCommand) -> CliResult<()> {
    match cmd {
        SessionCommand::List {
            limit,
            path,
            truncate,
        } => {
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
                    let last_update = s.updated_at.as_deref().unwrap_or("-");
                    let last_response = s.last_response_at.as_deref().unwrap_or("-");
                    let cwd = s.cwd.as_deref().unwrap_or("-");
                    let preview_raw = s
                        .first_user_message
                        .as_deref()
                        .unwrap_or("")
                        .replace('\n', " ");
                    let preview = if let Some(n) = truncate {
                        super::doctor::truncate_for_display(&preview_raw, n)
                    } else {
                        preview_raw
                    };

                    println!("- id: {}", s.id);
                    println!(
                        "  rounds: {} (user/assistant: {}/{}) | last_response: {} | last_update: {} | cwd: {}",
                        s.rounds, s.user_turns, s.assistant_turns, last_response, last_update, cwd
                    );
                    if !preview.is_empty() {
                        println!("  prompt: {}", preview);
                    }
                    println!();
                }
            }
        }
        SessionCommand::Recent {
            limit,
            since,
            raw_cwd,
            format,
            open,
            terminal,
            shell,
            keep_open,
            resume_cmd,
            wt_window,
            delay_ms,
            dry_run,
        } => {
            let sessions = find_recent_codex_sessions(since.into(), limit).await?;
            let mut rows: Vec<(String, String, Option<String>, u64)> =
                Vec::with_capacity(sessions.len());
            for s in sessions {
                let cwd_opt = s.cwd.clone();
                let cwd = cwd_opt.as_deref().unwrap_or("-");
                let root = if raw_cwd {
                    cwd.to_string()
                } else {
                    infer_project_root_from_cwd(cwd)
                };
                rows.push((root, s.id, cwd_opt, s.mtime_ms));
            }

            print_recent_sessions(format, &rows)?;

            if !open {
                return Ok(());
            }

            let term = terminal.unwrap_or_else(|| {
                if cfg!(windows) {
                    RecentTerminal::Wt
                } else {
                    RecentTerminal::Wezterm
                }
            });
            let shell = shell.unwrap_or_else(|| {
                if cfg!(windows) {
                    "pwsh".to_string()
                } else {
                    "sh".to_string()
                }
            });

            for (root, id, _cwd, _mtime_ms) in rows {
                let workdir = root.trim();
                if workdir.is_empty() || workdir == "-" {
                    eprintln!(" [跳过] 会话 cwd 不可用: {id}");
                    continue;
                }
                if !std::path::Path::new(workdir).exists() {
                    eprintln!(" [跳过] 目录不存在: {workdir}");
                    continue;
                }

                let full_cmd = render_resume_cmd(&resume_cmd, &id);
                match term {
                    RecentTerminal::Wt => {
                        if !cfg!(windows) {
                            return Err(crate::CliError::Other(
                                "--terminal wt is only supported on Windows".to_string(),
                            ));
                        }
                        spawn_windows_terminal_wt(
                            wt_window, workdir, &shell, keep_open, &full_cmd, dry_run,
                        )?;
                    }
                    RecentTerminal::Wezterm => {
                        spawn_wezterm(workdir, &shell, keep_open, &full_cmd, dry_run)?;
                    }
                }

                if delay_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
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
                println!("  rounds: {}", s.rounds);
                println!(
                    "  last_response_at: {}",
                    s.last_response_at.as_deref().unwrap_or("-")
                );
                println!(
                    "  last_update_at: {}",
                    s.updated_at.as_deref().unwrap_or("-")
                );
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
        SessionCommand::Transcript {
            id,
            all,
            tail,
            format,
            timestamps,
            path,
        } => {
            let session_opt: Option<SessionSummary> = if let Some(p) = path.as_deref() {
                let root = std::path::PathBuf::from(p);
                let sessions = find_codex_sessions_for_dir(&root, usize::MAX).await?;
                sessions.into_iter().find(|s| s.id == id)
            } else {
                let sessions = find_codex_sessions_for_current_dir(usize::MAX).await?;
                sessions.into_iter().find(|s| s.id == id)
            };

            let session_path = if let Some(sess) = session_opt.as_ref() {
                sess.path.clone()
            } else if let Some(found) = find_codex_session_file_by_id(&id).await? {
                found
            } else {
                println!("Session with id {} not found under ~/.codex/sessions", id);
                return Ok(());
            };

            let meta = read_codex_session_meta(&session_path).await?;
            println!("Codex session transcript:");
            println!("  id: {}", id);
            if let Some(meta) = meta.as_ref() {
                if let Some(cwd) = meta.cwd.as_deref() {
                    println!("  cwd: {}", cwd);
                }
                if let Some(ts) = meta.created_at.as_deref() {
                    println!("  created_at: {}", ts);
                }
            }
            println!("  file: {:?}", session_path);
            println!();

            let slice = if all { None } else { Some(tail) };
            let messages = read_codex_session_transcript(&session_path, slice).await?;

            let fmt = format.to_lowercase();
            if fmt == "json" {
                let json =
                    serde_json::to_string_pretty(&messages).unwrap_or_else(|_| "[]".to_string());
                println!("{json}");
                return Ok(());
            }

            if fmt == "markdown" {
                println!("# Codex session transcript\n");
                println!("- id: `{}`", id);
                if let Some(cwd) = meta.as_ref().and_then(|m| m.cwd.as_deref()) {
                    println!("- cwd: `{}`", cwd);
                }
                println!();
                for m in messages {
                    println!("## {}", m.role);
                    println!();
                    println!("{}", m.text);
                    println!();
                }
                return Ok(());
            }

            // Default: text
            for m in messages {
                if timestamps && let Some(ts) = m.timestamp.as_deref() {
                    println!("[{}] {}: {}", ts, m.role, m.text);
                    continue;
                }
                println!("{}: {}", m.role, m.text);
                println!();
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
                    let last_update = s.updated_at.as_deref().unwrap_or("-");
                    let last_response = s.last_response_at.as_deref().unwrap_or("-");
                    let cwd = s.cwd.as_deref().unwrap_or("-");
                    let preview_raw = s
                        .first_user_message
                        .as_deref()
                        .unwrap_or("")
                        .replace('\n', " ");
                    let preview = super::doctor::truncate_for_display(&preview_raw, 80);

                    println!("- id: {}", s.id);
                    println!(
                        "  rounds: {} (user/assistant: {}/{}) | last_response: {} | last_update: {} | cwd: {}",
                        s.rounds, s.user_turns, s.assistant_turns, last_response, last_update, cwd
                    );
                    if !preview.is_empty() {
                        println!("  prompt: {}", preview);
                    }
                    println!();
                }
            }
        }
        SessionCommand::Export { id, format, output } => {
            // For now, only lookup by scanning all sessions under current dir.
            let cwd = std::env::current_dir().map_err(|e| {
                crate::CliError::Other(format!("failed to resolve current directory: {e}"))
            })?;
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
                    "last_response_at": sess.last_response_at,
                    "user_turns": sess.user_turns,
                    "assistant_turns": sess.assistant_turns,
                    "rounds": sess.rounds,
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
                if let Some(updated) = sess.last_response_at.as_deref() {
                    md.push_str(&format!("- last_response_at: `{}`\n", updated));
                }
                md.push_str(&format!("- rounds: `{}`\n", sess.rounds));
                if let Some(cwd) = sess.cwd.as_deref() {
                    md.push_str(&format!("- cwd: `{}`\n", cwd));
                }
                md.push('\n');
                if let Some(msg) = sess.first_user_message.as_deref() {
                    md.push_str("## First user message\n\n");
                    md.push_str(msg);
                    md.push('\n');
                }
                md
            };

            if let Some(path) = output {
                let out_path = std::path::PathBuf::from(path);
                if let Some(parent) = out_path.parent()
                    && !parent.as_os_str().is_empty()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    return Err(crate::CliError::Other(format!(
                        "failed to create parent dir {:?}: {}",
                        parent, e
                    )));
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
