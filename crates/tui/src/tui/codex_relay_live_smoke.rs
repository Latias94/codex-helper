use std::time::Instant;

use tokio::sync::mpsc;

use crate::proxy::{
    CODEX_RELAY_LIVE_SMOKE_ACK, CodexRelayLiveSmokeCase, CodexRelayLiveSmokeRequest,
    CodexRelayLiveSmokeResponse, ProxyService,
};
use crate::tui::codex_relay_diagnostics::infer_codex_relay_diagnostics_model;
use crate::tui::i18n;
use crate::tui::model::Snapshot;
use crate::tui::state::UiState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum CodexRelayLiveSmokeMode {
    CompactOnly,
    CompactAndImage,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct CodexRelayLiveSmokeResult {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) result: Result<CodexRelayLiveSmokeResponse, String>,
}

pub(in crate::tui) type CodexRelayLiveSmokeSender =
    mpsc::UnboundedSender<CodexRelayLiveSmokeResult>;

pub(in crate::tui) fn confirm_or_request_codex_relay_live_smoke(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    tx: &CodexRelayLiveSmokeSender,
    mode: CodexRelayLiveSmokeMode,
) -> bool {
    if ui.service_name != "codex" {
        ui.toast = Some((
            i18n::label(
                ui.language,
                "Codex relay live smoke is only available for Codex service",
            )
            .to_string(),
            Instant::now(),
        ));
        return true;
    }

    if ui.codex_relay_live_smoke.loading {
        ui.toast = Some((
            i18n::label(ui.language, "Codex relay live smoke already running").to_string(),
            Instant::now(),
        ));
        return true;
    }

    let now = Instant::now();
    if !live_smoke_confirmation_is_current(ui, mode, now) {
        ui.codex_relay_live_smoke.pending_confirm = Some(mode);
        ui.codex_relay_live_smoke.pending_confirm_at = Some(now);
        ui.toast = Some((confirm_message(ui, mode).to_string(), now));
        return true;
    }

    ui.codex_relay_live_smoke.pending_confirm = None;
    ui.codex_relay_live_smoke.pending_confirm_at = None;
    request_codex_relay_live_smoke(ui, snapshot, proxy, tx, mode)
}

fn live_smoke_confirmation_is_current(
    ui: &UiState,
    mode: CodexRelayLiveSmokeMode,
    now: Instant,
) -> bool {
    ui.codex_relay_live_smoke.pending_confirm == Some(mode)
        && ui
            .codex_relay_live_smoke
            .pending_confirm_at
            .is_some_and(|prev| now.duration_since(prev) <= std::time::Duration::from_secs(3))
}

fn confirm_message(ui: &UiState, mode: CodexRelayLiveSmokeMode) -> &'static str {
    match (ui.language, mode) {
        (crate::tui::Language::Zh, CodexRelayLiveSmokeMode::CompactOnly) => {
            "再次按 X 运行真实 remote compaction smoke；会消耗上游 tokens/余额。"
        }
        (crate::tui::Language::Zh, CodexRelayLiveSmokeMode::CompactAndImage) => {
            "再次按 Y 运行真实 compact+image smoke；可能生成图片并消耗更多余额。"
        }
        (crate::tui::Language::En, CodexRelayLiveSmokeMode::CompactOnly) => {
            "Press X again to run live remote compaction smoke; this may consume upstream tokens or credits."
        }
        (crate::tui::Language::En, CodexRelayLiveSmokeMode::CompactAndImage) => {
            "Press Y again to run live compact+image smoke; this may generate an image and consume more credits."
        }
    }
}

fn request_codex_relay_live_smoke(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    tx: &CodexRelayLiveSmokeSender,
    mode: CodexRelayLiveSmokeMode,
) -> bool {
    let Some(model) = infer_codex_relay_diagnostics_model(ui, snapshot) else {
        ui.toast = Some((
            i18n::label(
                ui.language,
                "Codex relay live smoke needs an explicit model in a selected/recent session or default profile",
            )
            .to_string(),
            Instant::now(),
        ));
        return true;
    };

    let cases = match mode {
        CodexRelayLiveSmokeMode::CompactOnly => vec![CodexRelayLiveSmokeCase::ResponsesCompact],
        CodexRelayLiveSmokeMode::CompactAndImage => vec![
            CodexRelayLiveSmokeCase::ResponsesCompact,
            CodexRelayLiveSmokeCase::HostedImageGeneration,
        ],
    };
    let request = CodexRelayLiveSmokeRequest {
        acknowledgement: Some(CODEX_RELAY_LIVE_SMOKE_ACK.to_string()),
        model: Some(model),
        cases,
        ..CodexRelayLiveSmokeRequest::default()
    };

    let generation = ui.codex_relay_live_smoke.generation.saturating_add(1);
    ui.codex_relay_live_smoke.generation = generation;
    ui.codex_relay_live_smoke.loading = true;
    ui.codex_relay_live_smoke.mode = Some(mode);
    ui.codex_relay_live_smoke.last_started_at = Some(Instant::now());
    ui.codex_relay_live_smoke.last_error = None;
    ui.toast = Some((
        i18n::label(ui.language, "Codex relay live smoke started").to_string(),
        Instant::now(),
    ));

    let proxy = proxy.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let result = proxy
            .codex_relay_live_smoke(request)
            .await
            .map_err(|error| error.to_string());
        let _ = tx.send(CodexRelayLiveSmokeResult { generation, result });
    });

    true
}

pub(in crate::tui) fn apply_codex_relay_live_smoke_result(
    ui: &mut UiState,
    result: CodexRelayLiveSmokeResult,
) -> bool {
    if result.generation != ui.codex_relay_live_smoke.generation {
        return false;
    }

    ui.codex_relay_live_smoke.loading = false;
    ui.codex_relay_live_smoke.last_finished_at = Some(Instant::now());
    match result.result {
        Ok(response) => {
            let passed = response
                .results
                .iter()
                .filter(|result| result.outcome == crate::proxy::CodexRelayLiveSmokeOutcome::Passed)
                .count();
            let total = response.results.len();
            ui.codex_relay_live_smoke.last_error = None;
            ui.codex_relay_live_smoke.last_result = Some(response);
            ui.toast = Some((
                format!(
                    "{}: {passed}/{total}",
                    i18n::label(ui.language, "Codex relay live smoke")
                ),
                Instant::now(),
            ));
        }
        Err(error) => {
            ui.codex_relay_live_smoke.last_result = None;
            ui.codex_relay_live_smoke.last_error = Some(error.clone());
            ui.toast = Some((
                format!(
                    "{}: {error}",
                    i18n::label(ui.language, "Codex relay live smoke failed")
                ),
                Instant::now(),
            ));
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_relay_live_smoke_confirmation_requires_matching_mode() {
        let mut ui = UiState::default();
        let now = Instant::now();
        ui.codex_relay_live_smoke.pending_confirm = Some(CodexRelayLiveSmokeMode::CompactOnly);
        ui.codex_relay_live_smoke.pending_confirm_at = Some(now);

        assert!(live_smoke_confirmation_is_current(
            &ui,
            CodexRelayLiveSmokeMode::CompactOnly,
            now + std::time::Duration::from_secs(2)
        ));
        assert!(!live_smoke_confirmation_is_current(
            &ui,
            CodexRelayLiveSmokeMode::CompactAndImage,
            now + std::time::Duration::from_secs(2)
        ));
        assert!(!live_smoke_confirmation_is_current(
            &ui,
            CodexRelayLiveSmokeMode::CompactOnly,
            now + std::time::Duration::from_secs(4)
        ));
    }
}
