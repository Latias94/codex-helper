use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::proxy_home_dir;
use crate::logging::now_ms;

use super::{CodexRelayCapabilitiesResponse, CodexRelayLiveSmokeResponse};

const CODEX_RELAY_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const CODEX_RELAY_EVIDENCE_FILE: &str = "codex_relay_evidence.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRelayEvidenceKind {
    CapabilityDiagnostics,
    LiveSmoke,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRelayEvidenceEntry {
    pub schema_version: u32,
    pub evidence_id: String,
    pub timestamp_ms: u64,
    pub source: String,
    pub kind: CodexRelayEvidenceKind,
    pub service_name: String,
    pub station_name: String,
    pub upstream_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    pub upstream_base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexRelayEvidenceFilters {
    pub kind: Option<CodexRelayEvidenceKind>,
    pub station_name: Option<String>,
    pub model: Option<String>,
}

impl CodexRelayEvidenceFilters {
    pub fn matches(&self, entry: &CodexRelayEvidenceEntry) -> bool {
        if let Some(kind) = self.kind
            && entry.kind != kind
        {
            return false;
        }
        if let Some(station_name) = self.station_name.as_deref()
            && !contains_ignore_ascii_case(entry.station_name.as_str(), station_name)
        {
            return false;
        }
        if let Some(model) = self.model.as_deref()
            && !entry
                .model
                .as_deref()
                .is_some_and(|value| contains_ignore_ascii_case(value, model))
        {
            return false;
        }
        true
    }
}

pub fn codex_relay_evidence_path() -> PathBuf {
    proxy_home_dir()
        .join("logs")
        .join(CODEX_RELAY_EVIDENCE_FILE)
}

pub fn append_codex_relay_capabilities_evidence(
    response: &CodexRelayCapabilitiesResponse,
    source: &str,
) -> std::io::Result<CodexRelayEvidenceEntry> {
    let entry = capability_evidence_entry(response, source)?;
    append_codex_relay_evidence_entry(&codex_relay_evidence_path(), &entry)?;
    Ok(entry)
}

pub fn append_codex_relay_live_smoke_evidence(
    response: &CodexRelayLiveSmokeResponse,
    source: &str,
) -> std::io::Result<CodexRelayEvidenceEntry> {
    let entry = live_smoke_evidence_entry(response, source)?;
    append_codex_relay_evidence_entry(&codex_relay_evidence_path(), &entry)?;
    Ok(entry)
}

pub fn read_recent_codex_relay_evidence(
    limit: usize,
    filters: &CodexRelayEvidenceFilters,
) -> std::io::Result<Vec<CodexRelayEvidenceEntry>> {
    read_recent_codex_relay_evidence_from_path(&codex_relay_evidence_path(), limit, filters)
}

fn capability_evidence_entry(
    response: &CodexRelayCapabilitiesResponse,
    source: &str,
) -> std::io::Result<CodexRelayEvidenceEntry> {
    Ok(CodexRelayEvidenceEntry {
        schema_version: CODEX_RELAY_EVIDENCE_SCHEMA_VERSION,
        evidence_id: Uuid::new_v4().to_string(),
        timestamp_ms: now_ms(),
        source: normalize_source(source),
        kind: CodexRelayEvidenceKind::CapabilityDiagnostics,
        service_name: response.service_name.clone(),
        station_name: response.station_name.clone(),
        upstream_index: response.upstream_index,
        provider_id: response.provider_id.clone(),
        endpoint_id: response.endpoint_id.clone(),
        provider_endpoint_key: response.provider_endpoint_key.clone(),
        upstream_base_url: response.upstream_base_url.clone(),
        model: response.model.clone(),
        payload: serde_json::to_value(response).map_err(std::io::Error::other)?,
    })
}

fn live_smoke_evidence_entry(
    response: &CodexRelayLiveSmokeResponse,
    source: &str,
) -> std::io::Result<CodexRelayEvidenceEntry> {
    Ok(CodexRelayEvidenceEntry {
        schema_version: CODEX_RELAY_EVIDENCE_SCHEMA_VERSION,
        evidence_id: Uuid::new_v4().to_string(),
        timestamp_ms: now_ms(),
        source: normalize_source(source),
        kind: CodexRelayEvidenceKind::LiveSmoke,
        service_name: response.service_name.clone(),
        station_name: response.station_name.clone(),
        upstream_index: response.upstream_index,
        provider_id: response.provider_id.clone(),
        endpoint_id: response.endpoint_id.clone(),
        provider_endpoint_key: response.provider_endpoint_key.clone(),
        upstream_base_url: response.upstream_base_url.clone(),
        model: Some(response.requested_model.clone()),
        payload: serde_json::to_value(response).map_err(std::io::Error::other)?,
    })
}

fn append_codex_relay_evidence_entry(
    path: &Path,
    entry: &CodexRelayEvidenceEntry,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _guard = match evidence_lock().lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    };
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn read_recent_codex_relay_evidence_from_path(
    path: &Path,
    limit: usize,
    filters: &CodexRelayEvidenceFilters,
) -> std::io::Result<Vec<CodexRelayEvidenceEntry>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let reader = BufReader::new(file);
    let mut entries = reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<CodexRelayEvidenceEntry>(&line).ok())
        .filter(|entry| filters.matches(entry))
        .collect::<Vec<_>>();
    entries.reverse();
    entries.truncate(limit);
    Ok(entries)
}

fn evidence_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn normalize_source(source: &str) -> String {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        "proxy_service".to_string()
    } else {
        trimmed.to_string()
    }
}

fn contains_ignore_ascii_case(value: &str, needle: &str) -> bool {
    value
        .to_ascii_lowercase()
        .contains(needle.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_capability_profile::{
        CodexCapabilityProfile, CodexCapabilityProfileInput, CodexCapabilitySupport,
        CodexModelCatalogProfile, CodexPatchModeRecommendation, CodexPatchModeRecommendationInput,
    };
    use crate::codex_integration::CodexPatchMode;
    use crate::proxy::{
        CodexRelayCapabilitiesObserved, CodexRelayLiveSmokeCase, CodexRelayLiveSmokeConfidence,
        CodexRelayLiveSmokeOutcome, CodexRelayLiveSmokeResult, CodexRelayLiveSmokeSideEffect,
        CodexRelayProbeConfidence, CodexRelayProbeKind, CodexRelayProbeResult,
        CodexRelayProbeSupport,
    };

    fn temp_evidence_path() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "codex-helper-evidence-test-{}.jsonl",
            Uuid::new_v4()
        ));
        path
    }

    fn probe_result(
        kind: CodexRelayProbeKind,
        support: CodexRelayProbeSupport,
    ) -> CodexRelayProbeResult {
        CodexRelayProbeResult {
            kind,
            support,
            confidence: CodexRelayProbeConfidence::SuccessStatus,
            status_code: Some(200),
            response_shape: Some("ok".to_string()),
            translation_required: false,
            error_class: None,
            reason: "ok".to_string(),
        }
    }

    fn capabilities_response() -> CodexRelayCapabilitiesResponse {
        let model_catalog = CodexModelCatalogProfile::unknown("test");
        let expected =
            CodexCapabilityProfile::for_input(CodexCapabilityProfileInput::from_patch_mode(
                CodexPatchMode::OfficialImagegenBridge,
                model_catalog.clone(),
            ));
        let recommendation =
            CodexPatchModeRecommendation::for_input(CodexPatchModeRecommendationInput {
                current_patch_mode: CodexPatchMode::OfficialImagegenBridge,
                model_catalog,
                responses: CodexCapabilitySupport::Supported,
                responses_compact: CodexCapabilitySupport::Supported,
            });
        CodexRelayCapabilitiesResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            station_name: "input".to_string(),
            upstream_index: 0,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            upstream_base_url: "https://relay.example/v1".to_string(),
            patch_mode: CodexPatchMode::OfficialImagegenBridge,
            responses_websocket: false,
            model: Some("gpt-5.5".to_string()),
            expected,
            observed: CodexRelayCapabilitiesObserved {
                models: probe_result(
                    CodexRelayProbeKind::Models,
                    CodexRelayProbeSupport::Supported,
                ),
                responses: probe_result(
                    CodexRelayProbeKind::Responses,
                    CodexRelayProbeSupport::Supported,
                ),
                responses_compact: probe_result(
                    CodexRelayProbeKind::ResponsesCompact,
                    CodexRelayProbeSupport::Supported,
                ),
            },
            recommendation,
            mismatches: Vec::new(),
        }
    }

    fn live_smoke_response(model: &str) -> CodexRelayLiveSmokeResponse {
        CodexRelayLiveSmokeResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            station_name: "input".to_string(),
            upstream_index: 0,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            upstream_base_url: "https://relay.example/v1".to_string(),
            requested_model: model.to_string(),
            upstream_model: model.to_string(),
            cases: vec![CodexRelayLiveSmokeCase::ResponsesCompact],
            results: vec![CodexRelayLiveSmokeResult {
                case: CodexRelayLiveSmokeCase::ResponsesCompact,
                outcome: CodexRelayLiveSmokeOutcome::Passed,
                confidence: CodexRelayLiveSmokeConfidence::LiveOutputShape,
                side_effect: CodexRelayLiveSmokeSideEffect::LiveRequest,
                status_code: Some(200),
                response_shape: Some("compact_output".to_string()),
                output_items_seen: 1,
                image_generation_call_seen: false,
                image_result_present: false,
                accepted_by_responses: true,
                error_class: None,
                reason: "ok".to_string(),
            }],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn codex_relay_evidence_appends_and_reads_newest_first() {
        let path = temp_evidence_path();
        let capability = capability_evidence_entry(&capabilities_response(), "unit").unwrap();
        append_codex_relay_evidence_entry(&path, &capability).unwrap();
        let live = live_smoke_evidence_entry(&live_smoke_response("gpt-5.5"), "unit").unwrap();
        append_codex_relay_evidence_entry(&path, &live).unwrap();

        let entries =
            read_recent_codex_relay_evidence_from_path(&path, 10, &Default::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, CodexRelayEvidenceKind::LiveSmoke);
        assert_eq!(
            entries[1].kind,
            CodexRelayEvidenceKind::CapabilityDiagnostics
        );
        assert_eq!(entries[0].model.as_deref(), Some("gpt-5.5"));
        assert!(entries[0].payload.get("results").is_some());
    }

    #[test]
    fn codex_relay_evidence_filters_by_kind_station_and_model() {
        let path = temp_evidence_path();
        append_codex_relay_evidence_entry(
            &path,
            &live_smoke_evidence_entry(&live_smoke_response("gpt-5.5"), "unit").unwrap(),
        )
        .unwrap();
        append_codex_relay_evidence_entry(
            &path,
            &live_smoke_evidence_entry(&live_smoke_response("o3"), "unit").unwrap(),
        )
        .unwrap();
        append_codex_relay_evidence_entry(
            &path,
            &capability_evidence_entry(&capabilities_response(), "unit").unwrap(),
        )
        .unwrap();

        let entries = read_recent_codex_relay_evidence_from_path(
            &path,
            10,
            &CodexRelayEvidenceFilters {
                kind: Some(CodexRelayEvidenceKind::LiveSmoke),
                station_name: Some("inp".to_string()),
                model: Some("gpt".to_string()),
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, CodexRelayEvidenceKind::LiveSmoke);
        assert_eq!(entries[0].model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn codex_relay_evidence_missing_file_returns_empty() {
        let path = temp_evidence_path();
        let entries =
            read_recent_codex_relay_evidence_from_path(&path, 10, &Default::default()).unwrap();
        assert!(entries.is_empty());
    }
}
