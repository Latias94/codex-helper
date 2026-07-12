use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::policy_actions::PolicyAction;
use crate::runtime_identity::ProviderEndpointKey;

const POLICY_ACTION_LEDGER_SCHEMA_VERSION: u32 = 1;
const POLICY_ACTION_LEDGER_ENV: &str = "CODEX_HELPER_POLICY_ACTION_LEDGER";

pub(super) type PolicyActionMap = HashMap<String, HashMap<ProviderEndpointKey, Vec<PolicyAction>>>;

#[derive(Debug, Clone)]
pub(super) struct PolicyActionStore {
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPolicyActionLedger {
    schema_version: u32,
    updated_at_ms: u64,
    #[serde(default)]
    entries: Vec<PersistedPolicyActionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPolicyActionEntry {
    service_name: String,
    action: PolicyAction,
}

impl PolicyActionStore {
    pub(super) fn from_env() -> Self {
        if let Ok(value) = std::env::var(POLICY_ACTION_LEDGER_ENV) {
            let trimmed = value.trim();
            if trimmed.eq_ignore_ascii_case("off")
                || trimmed.eq_ignore_ascii_case("false")
                || trimmed == "0"
            {
                return Self { path: None };
            }
            if !trimmed.is_empty() {
                return Self {
                    path: Some(PathBuf::from(trimmed)),
                };
            }
        }

        #[cfg(test)]
        {
            if std::env::var("CODEX_HELPER_HOME")
                .ok()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
            {
                return Self { path: None };
            }
        }

        Self {
            path: Some(
                crate::config::proxy_home_dir()
                    .join("state")
                    .join("policy-actions.json"),
            ),
        }
    }

    pub(super) fn load(&self, now_ms: u64) -> (PolicyActionMap, bool) {
        let Some(path) = self.path.as_ref() else {
            return (HashMap::new(), false);
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (HashMap::new(), false);
            }
            Err(err) => {
                warn!(path = %path.display(), error = %err, "failed to read policy action ledger");
                return (HashMap::new(), false);
            }
        };
        let ledger = match serde_json::from_str::<PersistedPolicyActionLedger>(&text) {
            Ok(ledger) => ledger,
            Err(err) => {
                warn!(path = %path.display(), error = %err, "failed to parse policy action ledger");
                return (HashMap::new(), false);
            }
        };
        if ledger.schema_version != POLICY_ACTION_LEDGER_SCHEMA_VERSION {
            warn!(
                path = %path.display(),
                schema_version = ledger.schema_version,
                supported_schema_version = POLICY_ACTION_LEDGER_SCHEMA_VERSION,
                "ignoring unsupported policy action ledger schema"
            );
            return (HashMap::new(), false);
        }
        let mut entries = PolicyActionMap::new();
        let mut pruned = false;
        for entry in ledger.entries {
            if !entry.action.is_active_at(now_ms) {
                pruned = true;
                continue;
            }
            entries
                .entry(entry.service_name)
                .or_default()
                .entry(entry.action.provider_endpoint_key.clone())
                .or_default()
                .push(entry.action);
        }
        (entries, pruned)
    }

    pub(super) fn save_blocking(
        &self,
        entries: &PolicyActionMap,
        updated_at_ms: u64,
    ) -> std::io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let ledger = persisted_ledger(entries, updated_at_ms);
        let text = serde_json::to_string_pretty(&ledger).map_err(std::io::Error::other)?;
        match crate::file_replace::write_bytes_file(path, text.as_bytes()) {
            Ok(()) => Ok(()),
            Err(err) => recover_policy_action_candidate(path, text.as_bytes(), err),
        }
    }

    pub(super) async fn save(
        &self,
        entries: &PolicyActionMap,
        updated_at_ms: u64,
    ) -> std::io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let ledger = persisted_ledger(entries, updated_at_ms);
        let text = serde_json::to_string_pretty(&ledger).map_err(std::io::Error::other)?;
        match crate::file_replace::write_bytes_file_async(path, text.as_bytes()).await {
            Ok(()) => Ok(()),
            Err(err) => recover_policy_action_candidate_async(path, text.as_bytes(), err).await,
        }
    }
}

fn validate_policy_action_candidate(bytes: &[u8]) -> std::io::Result<()> {
    let ledger = serde_json::from_slice::<PersistedPolicyActionLedger>(bytes)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    if ledger.schema_version != POLICY_ACTION_LEDGER_SCHEMA_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unsupported policy action ledger schema {}; expected {}",
                ledger.schema_version, POLICY_ACTION_LEDGER_SCHEMA_VERSION
            ),
        ));
    }
    Ok(())
}

fn recover_policy_action_candidate(
    path: &Path,
    candidate: &[u8],
    error: crate::file_replace::AtomicWriteError,
) -> std::io::Result<()> {
    crate::file_replace::recover_uncertain_candidate(
        path,
        candidate,
        error,
        validate_policy_action_candidate,
    )
}

async fn recover_policy_action_candidate_async(
    path: &Path,
    candidate: &[u8],
    error: crate::file_replace::AtomicWriteError,
) -> std::io::Result<()> {
    crate::file_replace::recover_uncertain_candidate_async(
        path,
        candidate,
        error,
        validate_policy_action_candidate,
    )
    .await
}

fn persisted_ledger(entries: &PolicyActionMap, updated_at_ms: u64) -> PersistedPolicyActionLedger {
    PersistedPolicyActionLedger {
        schema_version: POLICY_ACTION_LEDGER_SCHEMA_VERSION,
        updated_at_ms,
        entries: entries
            .iter()
            .flat_map(|(service_name, per_service)| {
                per_service.values().flat_map(|actions| {
                    actions
                        .iter()
                        .cloned()
                        .map(|action| PersistedPolicyActionEntry {
                            service_name: service_name.clone(),
                            action,
                        })
                })
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestFile(PathBuf);

    impl TestFile {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "codex-helper-policy-action-store-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&directory).expect("create test directory");
            Self(directory.join("policy-actions.json"))
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            if let Some(parent) = self.0.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }

    fn uncertain_error(path: &Path) -> crate::file_replace::AtomicWriteError {
        crate::file_replace::AtomicWriteError::CommitStateUnknown {
            path: path.to_path_buf(),
            stage: "injected test replacement",
            source: std::io::Error::other("injected uncertainty"),
        }
    }

    #[test]
    fn uncertain_write_recovers_only_the_exact_valid_policy_candidate() {
        let path = TestFile::new();
        let candidate = serde_json::to_vec_pretty(&persisted_ledger(&HashMap::new(), 2))
            .expect("serialize candidate");
        std::fs::write(&path.0, &candidate).expect("write recovered candidate");

        recover_policy_action_candidate(&path.0, &candidate, uncertain_error(&path.0))
            .expect("exact valid candidate should recover");

        let old = serde_json::to_vec_pretty(&persisted_ledger(&HashMap::new(), 1))
            .expect("serialize old ledger");
        std::fs::write(&path.0, &old).expect("write old ledger");
        let err = recover_policy_action_candidate(&path.0, &candidate, uncertain_error(&path.0))
            .expect_err("different valid bytes must not recover as the candidate");
        assert!(err.to_string().contains("do not match the candidate"));
        assert_eq!(std::fs::read(&path.0).expect("read old ledger"), old);
    }

    #[test]
    fn uncertain_write_rejects_an_exact_unsupported_policy_schema() {
        let path = TestFile::new();
        let candidate = serde_json::to_vec_pretty(&PersistedPolicyActionLedger {
            schema_version: POLICY_ACTION_LEDGER_SCHEMA_VERSION + 1,
            updated_at_ms: 1,
            entries: Vec::new(),
        })
        .expect("serialize unsupported candidate");
        std::fs::write(&path.0, &candidate).expect("write unsupported candidate");

        let err = recover_policy_action_candidate(&path.0, &candidate, uncertain_error(&path.0))
            .expect_err("unsupported schema must not recover");
        assert!(
            err.to_string()
                .contains("unsupported policy action ledger schema")
        );
    }
}
