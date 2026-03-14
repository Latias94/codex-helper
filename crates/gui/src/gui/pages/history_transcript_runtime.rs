use super::components::history_transcript;
use super::history::{HistoryViewState, TranscriptLoad};
use super::*;

pub(in crate::gui::pages) fn cancel_transcript_load(state: &mut HistoryViewState) {
    if let Some(load) = state.transcript_load.take() {
        load.join.abort();
    }
}

pub(super) fn poll_transcript_loader(ctx: &mut PageCtx<'_>) {
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
                Err(error) => {
                    ctx.view.history.transcript_raw_messages.clear();
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = Some(error.to_string());
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

pub(super) fn select_session_and_reset_transcript(ctx: &mut PageCtx<'_>, idx: usize, id: String) {
    if ctx
        .view
        .history
        .external_focus
        .as_ref()
        .is_some_and(|focus| focus.summary.id != id)
    {
        ctx.view.history.external_focus = None;
    }
    ctx.view.history.selected_idx = idx;
    ctx.view.history.selected_id = Some(id);
    reset_transcript_view_after_session_switch(ctx);
}

pub(super) fn reset_transcript_view_after_session_switch(ctx: &mut PageCtx<'_>) {
    ctx.view.history.loaded_for = None;
    cancel_transcript_load(&mut ctx.view.history);
    ctx.view.history.transcript_raw_messages.clear();
    ctx.view.history.transcript_messages.clear();
    ctx.view.history.transcript_error = None;
    ctx.view.history.transcript_plain_key = None;
    ctx.view.history.transcript_plain_text.clear();
    ctx.view.history.transcript_selected_msg_idx = 0;
    ctx.view.history.transcript_scroll_to_msg_idx = None;
}

pub(super) fn ensure_transcript_loading(
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
        let result = crate::sessions::read_codex_session_transcript(&path, tail).await;
        let _ = tx.send((seq, result));
    });

    ctx.view.history.transcript_load = Some(TranscriptLoad { seq, key, rx, join });
}
