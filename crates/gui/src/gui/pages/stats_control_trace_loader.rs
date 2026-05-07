use super::components::console_layout::console_note;
use super::stats_control_trace_summary::control_trace_summary;
use super::view_state::{ControlTraceRecordState, ControlTraceSourceKind, StatsViewState};
use super::*;
use crate::gui::proxy_control::{ControlTraceDataSource, ProxyController};
use crate::logging::{ControlTraceDetail, ControlTraceLogEntry};

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

pub(super) fn refresh_control_trace_state(
    state: &mut StatsViewState,
    lang: Language,
    proxy: &ProxyController,
    rt: &tokio::runtime::Runtime,
) {
    let source = proxy.control_trace_source();
    apply_control_trace_source_state(state, source.as_ref());
    state.control_trace_loaded_signature = source.as_ref().map(ControlTraceDataSource::signature);

    match proxy.read_control_trace_entries(rt, state.control_trace_limit) {
        Ok(result) => {
            apply_control_trace_source_state(state, Some(&result.source));
            state.control_trace_loaded_signature = Some(result.source.signature());
            state.control_trace_entries = result
                .entries
                .iter()
                .map(|entry| control_trace_record_from_entry(entry, lang))
                .collect();
            state.control_trace_loaded_limit = state.control_trace_limit;
            state.control_trace_last_loaded_ms = Some(now_ms());
            state.control_trace_last_error = None;
        }
        Err(err) => {
            state.control_trace_entries.clear();
            state.control_trace_loaded_limit = state.control_trace_limit;
            state.control_trace_last_loaded_ms = Some(now_ms());
            state.control_trace_last_error = Some(err.to_string());
        }
    }
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
                model: Some("gpt-5".to_string()),
            }),
            payload: serde_json::json!({}),
        };

        let record = control_trace_record_from_entry(&entry, Language::En);

        assert_eq!(record.trace_id.as_deref(), Some("codex-7"));
        assert_eq!(record.request_id, Some(7));
    }
}
