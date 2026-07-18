use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::proxy_home_dir;
use crate::local_log_store::{LogRetention, append_line};
use crate::logging::now_ms;

use super::{CodexRelayCapabilitiesResponse, CodexRelayLiveSmokeResponse};

const CODEX_RELAY_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const CODEX_RELAY_EVIDENCE_FILE: &str = "codex_relay_evidence.jsonl";
const DEFAULT_CODEX_RELAY_EVIDENCE_MAX_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_CODEX_RELAY_EVIDENCE_MAX_FILES: usize = 10;

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
    pub provider_id: String,
    pub endpoint_id: String,
    pub provider_endpoint_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexRelayEvidenceFilters {
    pub kind: Option<CodexRelayEvidenceKind>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
}

impl CodexRelayEvidenceFilters {
    pub fn matches(&self, entry: &CodexRelayEvidenceEntry) -> bool {
        if let Some(kind) = self.kind
            && entry.kind != kind
        {
            return false;
        }
        if let Some(provider_id) = self.provider_id.as_deref()
            && !contains_ignore_ascii_case(entry.provider_id.as_str(), provider_id)
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
        provider_id: response.provider_id.clone(),
        endpoint_id: response.endpoint_id.clone(),
        provider_endpoint_key: response.provider_endpoint_key.clone(),
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
        provider_id: response.provider_id.clone(),
        endpoint_id: response.endpoint_id.clone(),
        provider_endpoint_key: response.provider_endpoint_key.clone(),
        model: Some(response.requested_model.clone()),
        payload: serde_json::to_value(response).map_err(std::io::Error::other)?,
    })
}

fn append_codex_relay_evidence_entry(
    path: &Path,
    entry: &CodexRelayEvidenceEntry,
) -> std::io::Result<()> {
    let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    let _guard = match evidence_lock().lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    };
    append_line(path, codex_relay_evidence_retention(), &line)
}

fn read_recent_codex_relay_evidence_from_path(
    path: &Path,
    limit: usize,
    filters: &CodexRelayEvidenceFilters,
) -> std::io::Result<Vec<CodexRelayEvidenceEntry>> {
    read_recent_codex_relay_evidence_from_path_with_retention(
        path,
        limit,
        filters,
        codex_relay_evidence_retention(),
    )
}

fn read_recent_codex_relay_evidence_from_path_with_retention(
    path: &Path,
    limit: usize,
    filters: &CodexRelayEvidenceFilters,
    retention: LogRetention,
) -> std::io::Result<Vec<CodexRelayEvidenceEntry>> {
    crate::local_log_store::repair_log(path, retention);
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

fn codex_relay_evidence_retention() -> LogRetention {
    static RETENTION: OnceLock<LogRetention> = OnceLock::new();
    *RETENTION.get_or_init(|| {
        LogRetention::from_env(
            "CODEX_HELPER_RELAY_EVIDENCE_LOG_MAX_BYTES",
            "CODEX_HELPER_RELAY_EVIDENCE_LOG_MAX_FILES",
            DEFAULT_CODEX_RELAY_EVIDENCE_MAX_BYTES,
            DEFAULT_CODEX_RELAY_EVIDENCE_MAX_FILES,
        )
    })
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
    use crate::codex_capability_profile::{CodexCapabilityDecision, CodexModelCatalogProfile};
    use crate::provider_catalog::ProviderAdapter;
    use crate::proxy::{
        CodexRelayCapabilitiesObserved, CodexRelayContinuityDiagnostics,
        CodexRelayContinuityDomainSummary, CodexRelayLiveSmokeCase, CodexRelayLiveSmokeConfidence,
        CodexRelayLiveSmokeOutcome, CodexRelayLiveSmokeResult, CodexRelayLiveSmokeSideEffect,
        CodexRelayProbeConfidence, CodexRelayProbeKind, CodexRelayProbeResult,
        CodexRelayProbeSupport, CodexRelayProviderContract,
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
            credential_readiness: None,
            status_code: Some(200),
            response_shape: Some("ok".to_string()),
            translation_required: false,
            error_class: None,
            reason: "ok".to_string(),
        }
    }

    fn capabilities_response() -> CodexRelayCapabilitiesResponse {
        let model_catalog = CodexModelCatalogProfile::unknown("test");
        let expected = CodexRelayProviderContract {
            provider_adapter: ProviderAdapter::OpenAiCompatible,
            catalog_revision: None,
            request_dialects: vec![
                "responses_http".to_string(),
                "responses_compact".to_string(),
                "responses_websocket".to_string(),
            ],
            model_catalog,
            responses: CodexCapabilityDecision::supported("test"),
            remote_compaction_v1: CodexCapabilityDecision::supported("test"),
            hosted_image_generation: CodexCapabilityDecision::unknown("test"),
            responses_websocket: CodexCapabilityDecision::unknown("test"),
            ultra_maps_to_max: CodexCapabilityDecision::unknown("test"),
            web_search: CodexCapabilityDecision::unknown("test"),
            apply_patch: CodexCapabilityDecision::unknown("test"),
            reasoning_summaries: CodexCapabilityDecision::unknown("test"),
        };
        CodexRelayCapabilitiesResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            provider_id: "input".to_string(),
            endpoint_id: "default".to_string(),
            provider_endpoint_key: "codex/input/default".to_string(),
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
            continuity: CodexRelayContinuityDiagnostics {
                selected_domain: CodexRelayContinuityDomainSummary {
                    key: "provider_endpoint:codex/input/default".to_string(),
                    explicit: false,
                },
                same_domain_endpoint_count: 1,
                configured_endpoint_count: 1,
                affinity_policy: Some("fallback-sticky".to_string()),
                can_state_bound_failover_within_domain: false,
                warnings: Vec::new(),
                recommendations: Vec::new(),
            },
            mismatches: Vec::new(),
        }
    }

    fn live_smoke_response(model: &str) -> CodexRelayLiveSmokeResponse {
        CodexRelayLiveSmokeResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            provider_id: "input".to_string(),
            endpoint_id: "default".to_string(),
            provider_endpoint_key: "codex/input/default".to_string(),
            requested_model: model.to_string(),
            upstream_model: model.to_string(),
            cases: vec![CodexRelayLiveSmokeCase::ResponsesCompact],
            results: vec![CodexRelayLiveSmokeResult {
                case: CodexRelayLiveSmokeCase::ResponsesCompact,
                outcome: CodexRelayLiveSmokeOutcome::Passed,
                confidence: CodexRelayLiveSmokeConfidence::LiveOutputShape,
                credential_readiness: None,
                side_effect: CodexRelayLiveSmokeSideEffect::LiveRequest,
                status_code: Some(200),
                response_shape: Some("compact_output".to_string()),
                output_items_seen: 1,
                compaction_output_seen: true,
                compaction_output_items_seen: 1,
                response_completed_seen: false,
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
        let serialized = serde_json::to_value(&entries[0]).expect("serialize evidence");
        assert!(serialized.get("station_name").is_none());
        assert!(serialized.get("upstream_index").is_none());
        assert!(serialized.get("upstream_base_url").is_none());
        assert_eq!(serialized["provider_id"], "input");
        assert_eq!(serialized["endpoint_id"], "default");
        assert_eq!(serialized["provider_endpoint_key"], "codex/input/default");
    }

    #[test]
    fn codex_relay_evidence_filters_by_kind_provider_and_model() {
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
                provider_id: Some("inp".to_string()),
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

    #[test]
    fn codex_relay_evidence_repairs_oversized_active_log_before_reading() {
        let path = temp_evidence_path();
        std::fs::write(&path, vec![b'x'; 32]).expect("seed oversized evidence log");

        let entries = read_recent_codex_relay_evidence_from_path_with_retention(
            &path,
            10,
            &Default::default(),
            LogRetention::new(16, 1),
        )
        .expect("read repaired evidence log");

        assert!(entries.is_empty());
        assert!(
            !path.exists(),
            "oversized active evidence log should be rotated away before reading"
        );
        assert!(
            crate::local_log_store::collect_rotated_logs(&path).is_empty(),
            "oversized rotated evidence log should be pruned by retention budget"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn codex_relay_evidence_uses_bounded_jsonl_rotation() {
        let path = temp_evidence_path();
        std::fs::write(&path, vec![b'x'; 32]).expect("seed oversized evidence log");
        let entry = capability_evidence_entry(&capabilities_response(), "unit").unwrap();

        append_line(
            &path,
            LogRetention::new(16, 2),
            &serde_json::to_string(&entry).unwrap(),
        )
        .expect("append evidence through bounded store");

        assert!(
            path.exists(),
            "bounded append should recreate active evidence log"
        );
        let rotated = crate::local_log_store::collect_rotated_logs(&path);
        assert_eq!(rotated.len(), 1);
        let rotated_name = rotated[0]
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("rotated evidence name");
        let expected_prefix = format!(
            "{}.",
            path.file_stem()
                .and_then(|name| name.to_str())
                .expect("active evidence stem")
        );
        assert!(rotated_name.starts_with(&expected_prefix));
        assert!(rotated_name.ends_with(".jsonl"));
        let _ = std::fs::remove_file(path);
        for file in rotated {
            let _ = std::fs::remove_file(file.path);
        }
    }
}
