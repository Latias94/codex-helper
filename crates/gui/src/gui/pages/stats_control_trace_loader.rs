use super::components::console_layout::console_note;
use super::stats_control_trace_summary::control_trace_summary;
use super::view_state::{
    ControlTraceLoad, ControlTraceRecordState, ControlTraceSourceKind, StatsViewState,
};
use super::*;
use crate::gui::proxy_control::ControlTraceDataSource;
use crate::logging::{ControlTraceDetail, ControlTraceLogEntry};
use std::sync::mpsc::TryRecvError;

pub(super) fn render_control_trace_source_summary(
    ui: &mut egui::Ui,
    lang: Language,
    state: &StatsViewState,
) {
    match state.control_trace_source_kind {
        ControlTraceSourceKind::LocalFile => {
            if let Some(detail) = state.control_trace_source_detail.as_deref() {
                ui.small(format!("path: {detail}"));
            }
        }
        ControlTraceSourceKind::AttachedApi => {
            if let Some(detail) = state.control_trace_source_detail.as_deref() {
                ui.small(format!(
                    "{}: {detail}",
                    pick(lang, "附着 API", "Attached API")
                ));
            }
            console_note(
                ui,
                pick(
                    lang,
                    "这里通过附着代理的管理 API 读取 control trace，因此即使当前设备无法访问远端主机文件系统，也能看到远端代理最近的控制链事件。",
                    "This panel reads control trace through the attached proxy admin API, so it can show recent remote control-plane events without direct filesystem access on this device.",
                ),
            );
        }
        ControlTraceSourceKind::AttachedFallbackLocal => {
            if let Some(detail) = state.control_trace_source_detail.as_deref() {
                ui.small(format!("path: {detail}"));
            }
            console_note(
                ui,
                pick(
                    lang,
                    "当前附着目标还没有暴露 control trace API，所以这里只能回退为读取本机的 control_trace.jsonl；这不代表远端代理主机上的日志。",
                    "The attached target does not expose a control-trace API yet, so this panel falls back to the local control_trace.jsonl on the current device; it does not represent the remote proxy host logs.",
                ),
            );
        }
    }
}

pub(super) fn poll_control_trace_loader(ctx: &mut PageCtx<'_>) {
    let current_signature = ctx.proxy.control_trace_source_signature();
    let current_limit = ctx.view.stats.control_trace_limit.clamp(20, 400);
    let Some(load) = ctx.view.stats.control_trace_load.as_mut() else {
        return;
    };

    match load.rx.try_recv() {
        Ok((seq, result)) => {
            if seq != load.seq
                || load.source_signature != current_signature
                || load.limit != current_limit
            {
                ctx.view.stats.control_trace_load = None;
                return;
            }

            let limit = load.limit;
            ctx.view.stats.control_trace_load = None;
            match result {
                Ok(result) => {
                    apply_control_trace_source_state(&mut ctx.view.stats, Some(&result.source));
                    ctx.view.stats.control_trace_loaded_signature = Some(result.source.signature());
                    ctx.view.stats.control_trace_loaded_limit = limit;
                    ctx.view.stats.control_trace_entries = result
                        .entries
                        .iter()
                        .map(|entry| control_trace_record_from_entry(entry, ctx.lang))
                        .collect();
                    ctx.view.stats.control_trace_last_loaded_ms = Some(now_ms());
                    ctx.view.stats.control_trace_last_error = None;
                }
                Err(err) => {
                    ctx.view.stats.control_trace_last_error = Some(err.to_string());
                }
            }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            ctx.view.stats.control_trace_load = None;
        }
    }
}

fn cancel_control_trace_load(state: &mut StatsViewState) {
    if let Some(load) = state.control_trace_load.take() {
        load.join.abort();
    }
}

pub(super) fn refresh_control_trace_state(ctx: &mut PageCtx<'_>, force: bool) {
    let source = ctx.proxy.control_trace_source();
    let requested_signature = source.as_ref().map(ControlTraceDataSource::signature);
    apply_control_trace_source_state(&mut ctx.view.stats, source.as_ref());

    let limit = ctx.view.stats.control_trace_limit.clamp(20, 400);
    ctx.view.stats.control_trace_limit = limit;

    if !force
        && ctx.view.stats.control_trace_load.is_none()
        && ctx.view.stats.control_trace_requested_signature == requested_signature
        && ctx.view.stats.control_trace_requested_limit == limit
    {
        return;
    }

    if force {
        cancel_control_trace_load(&mut ctx.view.stats);
    } else if ctx.view.stats.control_trace_load.is_some() {
        return;
    }

    ctx.view.stats.control_trace_requested_signature = requested_signature.clone();
    ctx.view.stats.control_trace_requested_limit = limit;
    ctx.view.stats.control_trace_last_error = None;

    let future = match ctx.proxy.read_control_trace_entries_task(limit) {
        Ok(future) => future,
        Err(err) => {
            ctx.view.stats.control_trace_last_error = Some(err.to_string());
            return;
        }
    };

    ctx.view.stats.control_trace_load_seq = ctx.view.stats.control_trace_load_seq.saturating_add(1);
    let seq = ctx.view.stats.control_trace_load_seq;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = future.await;
        let _ = tx.send((seq, result));
    });

    ctx.view.stats.control_trace_load = Some(ControlTraceLoad {
        seq,
        source_signature: requested_signature,
        limit,
        rx,
        join,
    });
}

fn apply_control_trace_source_state(
    state: &mut StatsViewState,
    source: Option<&ControlTraceDataSource>,
) {
    match source {
        Some(ControlTraceDataSource::LocalFile { path }) => {
            state.control_trace_source_kind = ControlTraceSourceKind::LocalFile;
            state.control_trace_source_detail = Some(path.display().to_string());
        }
        Some(ControlTraceDataSource::AttachedApi { admin_base_url }) => {
            state.control_trace_source_kind = ControlTraceSourceKind::AttachedApi;
            state.control_trace_source_detail = Some(admin_base_url.clone());
        }
        Some(ControlTraceDataSource::AttachedFallbackLocal { path, .. }) => {
            state.control_trace_source_kind = ControlTraceSourceKind::AttachedFallbackLocal;
            state.control_trace_source_detail = Some(path.display().to_string());
        }
        None => {
            state.control_trace_source_kind = ControlTraceSourceKind::LocalFile;
            state.control_trace_source_detail = None;
        }
    }
}

fn control_trace_record_from_entry(
    entry: &ControlTraceLogEntry,
    lang: Language,
) -> ControlTraceRecordState {
    ControlTraceRecordState {
        ts_ms: entry.ts_ms,
        kind: entry.kind.clone(),
        service: entry.service.clone(),
        request_id: entry.request_id,
        trace_id: entry.resolved_trace_id(),
        event: resolved_control_trace_event(entry),
        summary: control_trace_summary(entry, lang),
    }
}

fn resolved_control_trace_event(entry: &ControlTraceLogEntry) -> Option<String> {
    entry.event.clone().or_else(|| {
        entry.resolved_detail().map(|detail| match detail {
            ControlTraceDetail::RequestCompleted { .. } => "request_completed".to_string(),
            ControlTraceDetail::RetryOptions { .. } => "retry_options".to_string(),
            ControlTraceDetail::AttemptSelect { .. } => "attempt_select".to_string(),
            ControlTraceDetail::LoadBalancerSelection { .. } => "lbs_for_request".to_string(),
            ControlTraceDetail::ProviderRuntimeOverride { .. } => {
                "provider_runtime_override".to_string()
            }
            ControlTraceDetail::RouteExecutorShadowMismatch { .. } => {
                "route_executor_shadow_mismatch".to_string()
            }
            ControlTraceDetail::RouteGraphSelectionExplain { .. } => {
                "route_graph_selection_explain".to_string()
            }
            ControlTraceDetail::RetryEvent { event_name, .. } => event_name,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_trace_record_uses_resolved_trace_id() {
        let entry = ControlTraceLogEntry {
            ts_ms: 1,
            kind: "retry_trace".to_string(),
            service: Some("codex".to_string()),
            request_id: Some(7),
            trace_id: None,
            event: Some("attempt_select".to_string()),
            detail: Some(ControlTraceDetail::AttemptSelect {
                station_name: Some("right".to_string()),
                upstream_index: Some(0),
                upstream_base_url: None,
                provider_id: Some("right".to_string()),
                endpoint_id: Some("default".to_string()),
                provider_endpoint_key: Some("codex/right/default".to_string()),
                preference_group: Some(0),
                model: Some("gpt-5".to_string()),
            }),
            payload: serde_json::json!({}),
        };

        let record = control_trace_record_from_entry(&entry, Language::En);

        assert_eq!(record.trace_id.as_deref(), Some("codex-7"));
        assert_eq!(record.request_id, Some(7));
    }
}
