use std::path::{Path, PathBuf};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::sessions::{read_codex_session_meta, read_codex_session_transcript};
use crate::tui::i18n;
use crate::tui::state::UiState;
use crate::tui::types::Overlay;

pub(super) async fn handle_key_session_transcript(ui: &mut UiState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('t') => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Char('A') | KeyCode::Char('a') => {
            let Some(file) = ui.session_transcript_file.as_deref() else {
                ui.toast = Some((
                    i18n::label(ui.language, "no transcript file loaded").to_string(),
                    Instant::now(),
                ));
                return true;
            };

            ui.session_transcript_tail = match ui.session_transcript_tail {
                Some(_) => None,
                None => Some(80),
            };
            ui.session_transcript_messages.clear();
            ui.session_transcript_scroll = u16::MAX;
            ui.session_transcript_error = None;

            let path = PathBuf::from(file);
            match read_codex_session_transcript(&path, ui.session_transcript_tail).await {
                Ok(msgs) => {
                    ui.session_transcript_messages = msgs;
                    ui.toast = Some((
                        match ui.session_transcript_tail {
                            Some(n) => format!(
                                "{} {n}",
                                i18n::label(ui.language, "transcript: loaded tail")
                            ),
                            None => i18n::label(ui.language, "transcript: loaded all").to_string(),
                        },
                        Instant::now(),
                    ));
                }
                Err(e) => {
                    ui.session_transcript_error = Some(e.to_string());
                    ui.toast = Some((
                        format!(
                            "{}: {e}",
                            i18n::label(ui.language, "transcript: reload failed")
                        ),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('y') => {
            let text = format_session_transcript_text(ui);
            match super::try_copy_to_clipboard(&text) {
                Ok(()) => {
                    ui.toast = Some((
                        i18n::label(ui.language, "transcript: copied to clipboard").to_string(),
                        Instant::now(),
                    ))
                }
                Err(e) => {
                    ui.toast = Some((
                        format!(
                            "{}: {e}",
                            i18n::label(ui.language, "transcript: copy failed")
                        ),
                        Instant::now(),
                    ))
                }
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_add(1);
            true
        }
        KeyCode::PageUp => {
            ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_sub(10);
            true
        }
        KeyCode::PageDown => {
            ui.session_transcript_scroll = ui.session_transcript_scroll.saturating_add(10);
            true
        }
        KeyCode::Home | KeyCode::Char('g') => {
            ui.session_transcript_scroll = 0;
            true
        }
        KeyCode::End | KeyCode::Char('G') => {
            ui.session_transcript_scroll = u16::MAX;
            true
        }
        KeyCode::Char('L') => {
            super::toggle_language(ui).await;
            true
        }
        _ => false,
    }
}

fn format_session_transcript_text(ui: &UiState) -> String {
    let sid = ui.session_transcript_sid.as_deref().unwrap_or("-");
    let mode = match ui.session_transcript_tail {
        Some(n) => format!("tail {n}"),
        None => "all".to_string(),
    };

    let mut out = String::new();
    out.push_str(&format!("sid: {sid}\n"));
    out.push_str(&format!("mode: {mode}\n"));

    if let Some(meta) = ui.session_transcript_meta.as_ref() {
        out.push_str(&format!(
            "meta: id={} cwd={}\n",
            meta.id,
            meta.cwd.as_deref().unwrap_or("-")
        ));
    }
    if let Some(file) = ui.session_transcript_file.as_deref() {
        out.push_str(&format!("file: {file}\n"));
    }
    out.push('\n');

    for msg in ui.session_transcript_messages.iter() {
        let head = if let Some(ts) = msg.timestamp.as_deref() {
            format!("[{}] {}", ts, msg.role)
        } else {
            msg.role.clone()
        };
        out.push_str(&head);
        out.push('\n');
        out.push_str(msg.text.as_str());
        out.push_str("\n\n");
    }

    out
}

pub(super) async fn open_session_transcript_from_path(
    ui: &mut UiState,
    sid: String,
    path: &Path,
    tail: Option<usize>,
) {
    ui.session_transcript_sid = Some(sid);
    ui.session_transcript_meta = None;
    ui.session_transcript_file = Some(path.to_string_lossy().to_string());
    ui.session_transcript_tail = tail;
    ui.session_transcript_messages.clear();
    ui.session_transcript_scroll = u16::MAX;
    ui.session_transcript_error = None;

    match read_codex_session_meta(path).await {
        Ok(meta) => ui.session_transcript_meta = meta,
        Err(e) => ui.session_transcript_error = Some(e.to_string()),
    }
    match read_codex_session_transcript(path, tail).await {
        Ok(msgs) => ui.session_transcript_messages = msgs,
        Err(e) => ui.session_transcript_error = Some(e.to_string()),
    }
    ui.overlay = Overlay::SessionTranscript;
}
