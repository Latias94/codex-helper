use std::fmt;
use std::time::{Duration, Instant};

use crate::dashboard_core::OperatorReadModel;
use crate::proxy::{
    CODEX_RELAY_LIVE_SMOKE_ACK, CodexRelayCapabilitiesRequest, CodexRelayCapabilitiesResponse,
    CodexRelayLiveSmokeCase, CodexRelayLiveSmokeOutcome, CodexRelayLiveSmokeRequest,
    CodexRelayLiveSmokeResponse,
};

use super::model::Snapshot;

const LIVE_SMOKE_CONFIRM_WINDOW: Duration = Duration::from_secs(3);
const MAX_DISPLAY_ERROR_CHARS: usize = 512;
const GENERIC_RELAY_ERROR: &str = "relay operation failed";
const REDACTED_RELAY_ERROR: &str =
    "relay operation failed; sensitive upstream details were redacted";

/// Resolves the model visible to the Settings relay controls without reading
/// configuration or credentials outside the captured operator projection.
pub(in crate::tui) fn infer_codex_relay_model(
    snapshot: &Snapshot,
    selected_session_idx: usize,
    operator_read_model: Option<&OperatorReadModel>,
) -> Option<String> {
    let selected = snapshot.rows.get(selected_session_idx);
    first_non_empty([
        selected
            .and_then(|row| row.effective_model.as_ref())
            .map(|value| value.value.as_str()),
        selected.and_then(|row| row.last_model.as_deref()),
        snapshot
            .recent
            .iter()
            .find_map(|request| non_empty(request.model.as_deref())),
        operator_default_profile_model(operator_read_model),
    ])
}

fn operator_default_profile_model(operator_read_model: Option<&OperatorReadModel>) -> Option<&str> {
    let summary = &operator_read_model?.data.as_ref()?.summary;
    summary
        .runtime
        .default_profile_summary
        .as_ref()
        .and_then(|profile| non_empty(profile.model.as_deref()))
        .or_else(|| {
            let default_profile = non_empty(summary.runtime.default_profile.as_deref())?;
            summary
                .profiles
                .iter()
                .find(|profile| profile.name == default_profile)
                .and_then(|profile| non_empty(profile.model.as_deref()))
        })
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    (!value.is_empty()).then_some(value)
}

fn first_non_empty<const N: usize>(candidates: [Option<&str>; N]) -> Option<String> {
    candidates
        .into_iter()
        .find_map(non_empty)
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum CodexRelayActionBlock {
    CodexServiceOnly,
    AlreadyRunning,
    ExplicitModelRequired,
}

impl fmt::Display for CodexRelayActionBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CodexServiceOnly => "Codex relay controls are only available for Codex",
            Self::AlreadyRunning => "a Codex relay operation is already running",
            Self::ExplicitModelRequired => {
                "live smoke needs a model from the selected session, recent requests, or default profile"
            }
        })
    }
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct CodexRelayDiagnosticsStart {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) request: CodexRelayCapabilitiesRequest,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct CodexRelayDiagnosticsCompletion {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) result: Result<CodexRelayCapabilitiesResponse, String>,
}

#[derive(Debug, Clone, Default)]
pub(in crate::tui) struct CodexRelayDiagnosticsState {
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) last_started_at: Option<Instant>,
    pub(in crate::tui) last_finished_at: Option<Instant>,
    pub(in crate::tui) last_result: Option<CodexRelayCapabilitiesResponse>,
    pub(in crate::tui) last_error: Option<String>,
}

impl CodexRelayDiagnosticsState {
    /// Starts a read-only capability diagnostic. Execution is deliberately left
    /// to the caller so integrated and signed-loopback transports share state.
    pub(in crate::tui) fn begin(
        &mut self,
        service_name: &str,
        model: Option<String>,
        now: Instant,
    ) -> Result<CodexRelayDiagnosticsStart, CodexRelayActionBlock> {
        if service_name != "codex" {
            return Err(CodexRelayActionBlock::CodexServiceOnly);
        }
        if self.loading {
            return Err(CodexRelayActionBlock::AlreadyRunning);
        }

        let generation = self.generation.saturating_add(1);
        self.loading = true;
        self.generation = generation;
        self.last_started_at = Some(now);
        self.last_error = None;

        Ok(CodexRelayDiagnosticsStart {
            generation,
            request: CodexRelayCapabilitiesRequest {
                model: model.and_then(|model| {
                    let model = model.trim();
                    (!model.is_empty()).then(|| model.to_owned())
                }),
                ..CodexRelayCapabilitiesRequest::default()
            },
        })
    }

    /// Returns false when a completion belongs to an obsolete generation.
    pub(in crate::tui) fn apply_completion(
        &mut self,
        completion: CodexRelayDiagnosticsCompletion,
        now: Instant,
    ) -> bool {
        if completion.generation != self.generation {
            return false;
        }

        self.loading = false;
        self.last_finished_at = Some(now);
        match completion.result {
            Ok(response) => {
                self.last_error = None;
                self.last_result = Some(response);
            }
            Err(error) => {
                self.last_result = None;
                self.last_error = Some(sanitize_relay_display_error(&error));
            }
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum CodexRelayLiveSmokeMode {
    CompactOnly,
    CompactAndImage,
}

impl CodexRelayLiveSmokeMode {
    pub(in crate::tui) fn key(self) -> char {
        match self {
            Self::CompactOnly => 'X',
            Self::CompactAndImage => 'Y',
        }
    }

    fn cases(self) -> Vec<CodexRelayLiveSmokeCase> {
        match self {
            Self::CompactOnly => vec![CodexRelayLiveSmokeCase::ResponsesCompact],
            Self::CompactAndImage => vec![
                CodexRelayLiveSmokeCase::ResponsesCompact,
                CodexRelayLiveSmokeCase::HostedImageGeneration,
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct CodexRelayLiveSmokeStart {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) request: CodexRelayLiveSmokeRequest,
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct CodexRelayLiveSmokeCompletion {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) result: Result<CodexRelayLiveSmokeResponse, String>,
}

#[derive(Debug, Clone)]
pub(in crate::tui) enum CodexRelayLiveSmokeDecision {
    ConfirmAgain { mode: CodexRelayLiveSmokeMode },
    Started(CodexRelayLiveSmokeStart),
    Blocked(CodexRelayActionBlock),
}

#[derive(Debug, Clone, Default)]
pub(in crate::tui) struct CodexRelayLiveSmokeState {
    pub(in crate::tui) loading: bool,
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) mode: Option<CodexRelayLiveSmokeMode>,
    pub(in crate::tui) pending_confirm: Option<CodexRelayLiveSmokeMode>,
    pub(in crate::tui) pending_confirm_at: Option<Instant>,
    pub(in crate::tui) last_started_at: Option<Instant>,
    pub(in crate::tui) last_finished_at: Option<Instant>,
    pub(in crate::tui) last_result: Option<CodexRelayLiveSmokeResponse>,
    pub(in crate::tui) last_error: Option<String>,
}

impl CodexRelayLiveSmokeState {
    /// Requires the same mode to be requested twice within three seconds before
    /// returning a request that can cause billable upstream side effects.
    pub(in crate::tui) fn confirm_or_begin(
        &mut self,
        service_name: &str,
        model: Option<String>,
        mode: CodexRelayLiveSmokeMode,
        now: Instant,
    ) -> CodexRelayLiveSmokeDecision {
        if service_name != "codex" {
            return CodexRelayLiveSmokeDecision::Blocked(CodexRelayActionBlock::CodexServiceOnly);
        }
        if self.loading {
            return CodexRelayLiveSmokeDecision::Blocked(CodexRelayActionBlock::AlreadyRunning);
        }

        if !self.confirmation_is_current(mode, now) {
            self.pending_confirm = Some(mode);
            self.pending_confirm_at = Some(now);
            return CodexRelayLiveSmokeDecision::ConfirmAgain { mode };
        }

        self.clear_confirmation();
        let Some(model) = model.and_then(|model| {
            let model = model.trim();
            (!model.is_empty()).then(|| model.to_owned())
        }) else {
            return CodexRelayLiveSmokeDecision::Blocked(
                CodexRelayActionBlock::ExplicitModelRequired,
            );
        };

        let generation = self.generation.saturating_add(1);
        self.loading = true;
        self.generation = generation;
        self.mode = Some(mode);
        self.last_started_at = Some(now);
        self.last_error = None;

        CodexRelayLiveSmokeDecision::Started(CodexRelayLiveSmokeStart {
            generation,
            request: CodexRelayLiveSmokeRequest {
                acknowledgement: Some(CODEX_RELAY_LIVE_SMOKE_ACK.to_string()),
                model: Some(model),
                cases: mode.cases(),
                ..CodexRelayLiveSmokeRequest::default()
            },
        })
    }

    pub(in crate::tui) fn clear_confirmation(&mut self) {
        self.pending_confirm = None;
        self.pending_confirm_at = None;
    }

    /// Returns false when a completion belongs to an obsolete generation.
    pub(in crate::tui) fn apply_completion(
        &mut self,
        completion: CodexRelayLiveSmokeCompletion,
        now: Instant,
    ) -> bool {
        if completion.generation != self.generation {
            return false;
        }

        self.loading = false;
        self.last_finished_at = Some(now);
        match completion.result {
            Ok(response) => {
                self.last_error = None;
                self.last_result = Some(response);
            }
            Err(error) => {
                self.last_result = None;
                self.last_error = Some(sanitize_relay_display_error(&error));
            }
        }
        true
    }

    pub(in crate::tui) fn passed_counts(&self) -> Option<(usize, usize)> {
        let response = self.last_result.as_ref()?;
        let passed = response
            .results
            .iter()
            .filter(|result| result.outcome == CodexRelayLiveSmokeOutcome::Passed)
            .count();
        Some((passed, response.results.len()))
    }

    fn confirmation_is_current(&self, mode: CodexRelayLiveSmokeMode, now: Instant) -> bool {
        self.pending_confirm == Some(mode)
            && self.pending_confirm_at.is_some_and(|confirmed_at| {
                now.checked_duration_since(confirmed_at)
                    .is_some_and(|elapsed| elapsed <= LIVE_SMOKE_CONFIRM_WINDOW)
            })
    }
}

/// Keeps displayable transport context while conservatively discarding text
/// that could contain authentication material or URL query credentials.
pub(in crate::tui) fn sanitize_relay_display_error(error: &str) -> String {
    let cleaned = error
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let error = cleaned.trim();
    if error.is_empty() {
        return GENERIC_RELAY_ERROR.to_string();
    }
    let lowercase = error.to_ascii_lowercase();
    const SENSITIVE_MARKERS: &[&str] = &[
        "authorization",
        "bearer ",
        "x-api-key",
        "x-goog-api-key",
        "api_key",
        "api-key",
        "auth_token",
        "access_token",
        "token=",
        "token:",
        "secret=",
        "secret:",
        "sk-",
    ];
    if SENSITIVE_MARKERS
        .iter()
        .any(|marker| lowercase.contains(marker))
    {
        return REDACTED_RELAY_ERROR.to_string();
    }

    let without_queries = error
        .split_whitespace()
        .map(|part| {
            if part.contains("://") {
                part.split_once('?').map_or(part, |(origin, _)| origin)
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(&without_queries, MAX_DISPLAY_ERROR_CHARS)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub("...[truncated]".len());
    let mut output = value.chars().take(keep).collect::<String>();
    output.push_str("...[truncated]");
    output
}

#[cfg(test)]
mod tests {
    use crate::state::{
        ResolvedRouteValue, RouteValueSource, SessionBindingProjection, SessionObservationScope,
    };

    use super::*;
    use crate::tui::model::SessionRow;

    fn session_row(effective_model: Option<&str>, last_model: Option<&str>) -> SessionRow {
        SessionRow {
            session_id: Some("session:opaque".to_string()),
            local_session_id: None,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            active_last_method: None,
            active_last_path: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: last_model.map(ToOwned::to_owned),
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            last_output_tokens_per_second: None,
            avg_output_tokens_per_second: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            binding: SessionBindingProjection::default(),
            last_route_decision: None,
            route_affinity: None,
            effective_model: effective_model.map(|model| ResolvedRouteValue {
                value: model.to_string(),
                source: RouteValueSource::RequestPayload,
            }),
            effective_reasoning_effort: None,
            effective_service_tier: None,
        }
    }

    #[test]
    fn model_inference_prefers_selected_effective_then_last_model() {
        let mut snapshot = Snapshot::default();
        snapshot
            .rows
            .push(session_row(Some(" gpt-effective "), Some("gpt-last")));

        assert_eq!(
            infer_codex_relay_model(&snapshot, 0, None).as_deref(),
            Some("gpt-effective")
        );

        snapshot.rows[0].effective_model = None;
        assert_eq!(
            infer_codex_relay_model(&snapshot, 0, None).as_deref(),
            Some("gpt-last")
        );
    }

    #[test]
    fn diagnostics_builds_read_only_request_and_sanitizes_errors() {
        let now = Instant::now();
        let mut state = CodexRelayDiagnosticsState::default();
        let start = state
            .begin("codex", Some(" gpt-5.6 ".to_string()), now)
            .expect("Codex diagnostics should start");

        assert_eq!(start.request.model.as_deref(), Some("gpt-5.6"));
        assert!(start.request.provider_id.is_none());
        assert!(start.request.endpoint_id.is_none());
        assert!(state.apply_completion(
            CodexRelayDiagnosticsCompletion {
                generation: start.generation,
                result: Err("status=401 Authorization: Bearer top-secret".to_string()),
            },
            now + Duration::from_millis(1),
        ));
        assert_eq!(state.last_error.as_deref(), Some(REDACTED_RELAY_ERROR));
    }

    #[test]
    fn diagnostics_ignores_obsolete_generation() {
        let now = Instant::now();
        let mut state = CodexRelayDiagnosticsState::default();
        let first = state
            .begin("codex", None, now)
            .expect("first diagnostic should start");
        assert!(state.apply_completion(
            CodexRelayDiagnosticsCompletion {
                generation: first.generation,
                result: Err("first failure".to_string()),
            },
            now,
        ));
        let second = state
            .begin("codex", None, now)
            .expect("second diagnostic should start");

        assert!(!state.apply_completion(
            CodexRelayDiagnosticsCompletion {
                generation: first.generation,
                result: Err("obsolete failure".to_string()),
            },
            now,
        ));
        assert_eq!(state.generation, second.generation);
        assert!(state.loading);
    }

    #[test]
    fn live_smoke_requires_matching_confirmation_and_builds_exact_cases() {
        let now = Instant::now();
        let mut state = CodexRelayLiveSmokeState::default();
        assert!(matches!(
            state.confirm_or_begin(
                "codex",
                Some("gpt-5.6".to_string()),
                CodexRelayLiveSmokeMode::CompactOnly,
                now,
            ),
            CodexRelayLiveSmokeDecision::ConfirmAgain {
                mode: CodexRelayLiveSmokeMode::CompactOnly,
                ..
            }
        ));

        assert!(matches!(
            state.confirm_or_begin(
                "codex",
                Some("gpt-5.6".to_string()),
                CodexRelayLiveSmokeMode::CompactAndImage,
                now + Duration::from_secs(1),
            ),
            CodexRelayLiveSmokeDecision::ConfirmAgain {
                mode: CodexRelayLiveSmokeMode::CompactAndImage,
                ..
            }
        ));

        let CodexRelayLiveSmokeDecision::Started(start) = state.confirm_or_begin(
            "codex",
            Some(" gpt-5.6 ".to_string()),
            CodexRelayLiveSmokeMode::CompactAndImage,
            now + Duration::from_secs(2),
        ) else {
            panic!("matching confirmation should start live smoke");
        };
        assert_eq!(
            start.request.acknowledgement.as_deref(),
            Some(CODEX_RELAY_LIVE_SMOKE_ACK)
        );
        assert_eq!(start.request.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(
            start.request.cases,
            vec![
                CodexRelayLiveSmokeCase::ResponsesCompact,
                CodexRelayLiveSmokeCase::HostedImageGeneration,
            ]
        );
    }

    #[test]
    fn compact_live_smoke_builds_only_responses_compact() {
        let now = Instant::now();
        let mut state = CodexRelayLiveSmokeState::default();
        let first = state.confirm_or_begin(
            "codex",
            Some("gpt-5.6".to_string()),
            CodexRelayLiveSmokeMode::CompactOnly,
            now,
        );
        assert!(matches!(
            first,
            CodexRelayLiveSmokeDecision::ConfirmAgain { .. }
        ));

        let CodexRelayLiveSmokeDecision::Started(start) = state.confirm_or_begin(
            "codex",
            Some("gpt-5.6".to_string()),
            CodexRelayLiveSmokeMode::CompactOnly,
            now + Duration::from_secs(3),
        ) else {
            panic!("confirmation at the boundary should start compact live smoke");
        };
        assert_eq!(
            start.request.acknowledgement.as_deref(),
            Some(CODEX_RELAY_LIVE_SMOKE_ACK)
        );
        assert_eq!(
            start.request.cases,
            vec![CodexRelayLiveSmokeCase::ResponsesCompact]
        );
    }

    #[test]
    fn live_smoke_confirmation_expires_after_three_seconds() {
        let now = Instant::now();
        let mut state = CodexRelayLiveSmokeState::default();
        let first = state.confirm_or_begin(
            "codex",
            Some("gpt-5.6".to_string()),
            CodexRelayLiveSmokeMode::CompactOnly,
            now,
        );
        assert!(matches!(
            first,
            CodexRelayLiveSmokeDecision::ConfirmAgain { .. }
        ));

        let expired = state.confirm_or_begin(
            "codex",
            Some("gpt-5.6".to_string()),
            CodexRelayLiveSmokeMode::CompactOnly,
            now + Duration::from_secs(4),
        );
        assert!(matches!(
            expired,
            CodexRelayLiveSmokeDecision::ConfirmAgain { .. }
        ));
        assert!(!state.loading);
    }

    #[test]
    fn display_error_strips_url_queries_and_limits_length() {
        let error = format!(
            "upstream https://relay.example/fail?credential=value {}",
            "x".repeat(700)
        );
        let sanitized = sanitize_relay_display_error(&error);

        assert!(!sanitized.contains("credential=value"));
        assert!(sanitized.chars().count() <= MAX_DISPLAY_ERROR_CHARS);
        assert!(sanitized.ends_with("...[truncated]"));
    }

    #[test]
    fn display_error_removes_terminal_controls_and_handles_empty_input() {
        assert_eq!(sanitize_relay_display_error("\0\n\r"), GENERIC_RELAY_ERROR);
        assert_eq!(
            sanitize_relay_display_error("upstream\u{1b}[31m failed"),
            "upstream [31m failed"
        );
    }
}
