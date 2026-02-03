use eframe::egui;

use super::super::super::i18n::{Language, pick};
use super::super::history::{HistoryViewState, TranscriptViewMode};
use super::super::shorten;
use crate::sessions::SessionTranscriptMessage;

pub(in super::super) fn render_transcript_body(
    ui: &mut egui::Ui,
    lang: Language,
    state: &mut HistoryViewState,
    max_height: f32,
) {
    if state.transcript_load.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(pick(lang, "加载中…", "Loading..."));
        });
        ui.add_space(6.0);
    }

    if state.transcript_messages.is_empty() {
        ui.label(pick(lang, "（无内容）", "(empty)"));
        return;
    }

    let mut find_query_changed = false;
    ui.horizontal(|ui| {
        ui.label(pick(lang, "查找", "Find"));
        let resp = ui.add(
            egui::TextEdit::singleline(&mut state.transcript_find_query)
                .desired_width(220.0)
                .hint_text(pick(
                    lang,
                    "关键词（仅已加载内容）",
                    "keyword (loaded only)",
                )),
        );
        find_query_changed |= resp.changed();

        find_query_changed |= ui
            .checkbox(
                &mut state.transcript_find_case_sensitive,
                pick(lang, "区分大小写", "Case"),
            )
            .changed();

        if ui.button(pick(lang, "清空", "Clear")).clicked() {
            state.transcript_find_query.clear();
            find_query_changed = true;
        }
    });

    let find_query = state.transcript_find_query.trim().to_string();
    let find_case_sensitive = state.transcript_find_case_sensitive;
    let matches = if find_query.is_empty() {
        Vec::new()
    } else {
        transcript_find_matches(
            state.transcript_messages.as_slice(),
            &find_query,
            find_case_sensitive,
        )
    };

    if !find_query.is_empty() {
        ui.horizontal(|ui| {
            let total = matches.len();
            ui.label(format!("{}: {}", pick(lang, "匹配", "Matches"), total));

            let enabled = total > 0;
            if ui
                .add_enabled(enabled, egui::Button::new(pick(lang, "上一处", "Prev")))
                .clicked()
            {
                if state.transcript_view != TranscriptViewMode::Messages {
                    state.transcript_view = TranscriptViewMode::Messages;
                }
                let current = state
                    .transcript_selected_msg_idx
                    .min(state.transcript_messages.len().saturating_sub(1));
                let target = matches
                    .iter()
                    .rev()
                    .copied()
                    .find(|&i| i < current)
                    .unwrap_or_else(|| *matches.last().unwrap());
                state.transcript_selected_msg_idx = target;
                state.transcript_scroll_to_msg_idx = Some(target);
            }

            if ui
                .add_enabled(enabled, egui::Button::new(pick(lang, "下一处", "Next")))
                .clicked()
            {
                if state.transcript_view != TranscriptViewMode::Messages {
                    state.transcript_view = TranscriptViewMode::Messages;
                }
                let current = state
                    .transcript_selected_msg_idx
                    .min(state.transcript_messages.len().saturating_sub(1));
                let target = matches
                    .iter()
                    .copied()
                    .find(|&i| i > current)
                    .unwrap_or(matches[0]);
                state.transcript_selected_msg_idx = target;
                state.transcript_scroll_to_msg_idx = Some(target);
            }

            if find_query_changed && enabled {
                let current = state
                    .transcript_selected_msg_idx
                    .min(state.transcript_messages.len().saturating_sub(1));
                if matches.binary_search(&current).is_ok() {
                    state.transcript_scroll_to_msg_idx = Some(current);
                }
            }
        });
        ui.add_space(6.0);
    }

    match state.transcript_view {
        TranscriptViewMode::Messages => {
            let list_h = (max_height * 0.45).clamp(140.0, 260.0);
            let total = state.transcript_messages.len();
            let row_h = 22.0;

            let mut scroll = egui::ScrollArea::vertical()
                .id_salt("history_transcript_messages_scroll")
                .max_height(list_h);
            if let Some(target) = state.transcript_scroll_to_msg_idx.take() {
                let offset = (row_h * target as f32 - list_h * 0.5).max(0.0);
                scroll = scroll.vertical_scroll_offset(offset);
            }

            scroll.show_rows(ui, row_h, total, |ui, range| {
                for i in range {
                    let selected = i == state.transcript_selected_msg_idx;
                    let (ts, role, preview) = {
                        let m = &state.transcript_messages[i];
                        let ts = m.timestamp.as_deref().unwrap_or("-");
                        let role = m.role.as_str();
                        let first_line = m.text.lines().next().unwrap_or("");
                        let preview = first_line.replace('\t', " ");
                        (ts.to_string(), role.to_string(), preview)
                    };
                    let label = format!(
                        "#{:>4}  {}  {}  {}",
                        i.saturating_add(1),
                        shorten(&ts, 19),
                        shorten(&role, 10),
                        shorten(&preview, 60)
                    );
                    let is_match = !find_query.is_empty() && matches.binary_search(&i).is_ok();
                    let text = if is_match {
                        egui::RichText::new(label).color(egui::Color32::from_rgb(220, 170, 60))
                    } else {
                        egui::RichText::new(label)
                    };
                    if ui.selectable_label(selected, text).clicked() {
                        state.transcript_selected_msg_idx = i;
                    }
                }
            });

            ui.add_space(6.0);

            if total == 0 {
                return;
            }
            let idx = state
                .transcript_selected_msg_idx
                .min(total.saturating_sub(1));
            state.transcript_selected_msg_idx = idx;

            let (ts, role) = {
                let m = &state.transcript_messages[idx];
                (
                    m.timestamp.clone().unwrap_or_else(|| "-".to_string()),
                    m.role.clone(),
                )
            };
            ui.label(format!("[{ts}] {role}:"));
            ui.add(
                egui::TextEdit::multiline(&mut state.transcript_messages[idx].text)
                    .desired_rows(6)
                    .font(egui::TextStyle::Monospace)
                    .interactive(false),
            );
        }
        TranscriptViewMode::PlainText => {
            let cache_key = state
                .loaded_for
                .clone()
                .map(|(id, tail)| (id, tail, state.hide_tool_calls));
            if let Some(k) = cache_key.clone()
                && state.transcript_plain_key.as_ref() != Some(&k)
            {
                state.transcript_plain_text.clear();
                for msg in state.transcript_messages.iter() {
                    let ts = msg.timestamp.as_deref().unwrap_or("-");
                    let role = msg.role.as_str();
                    state
                        .transcript_plain_text
                        .push_str(&format!("[{ts}] {role}:\n"));
                    state.transcript_plain_text.push_str(&msg.text);
                    state.transcript_plain_text.push_str("\n\n");
                }
                state.transcript_plain_key = Some(k);
            }

            egui::ScrollArea::vertical()
                .id_salt("history_transcript_plain_scroll")
                .max_height(max_height)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut state.transcript_plain_text)
                            .desired_rows(18)
                            .font(egui::TextStyle::Monospace)
                            .interactive(false),
                    );
                });
        }
    }
}

pub(in super::super) fn filter_tool_calls(
    mut msgs: Vec<SessionTranscriptMessage>,
    hide_tool_calls: bool,
) -> Vec<SessionTranscriptMessage> {
    if !hide_tool_calls {
        return msgs;
    }
    msgs.retain(|m| {
        let role = m.role.trim().to_ascii_lowercase();
        role != "tool" && role != "tools" && role != "function"
    });
    msgs
}

fn transcript_find_matches(
    msgs: &[SessionTranscriptMessage],
    query: &str,
    case_sensitive: bool,
) -> Vec<usize> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    if case_sensitive {
        msgs.iter()
            .enumerate()
            .filter(|(_, m)| m.text.contains(q))
            .map(|(i, _)| i)
            .collect()
    } else {
        let needle = q.to_ascii_lowercase();
        msgs.iter()
            .enumerate()
            .filter(|(_, m)| m.text.to_ascii_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }
}
