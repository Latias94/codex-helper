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
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let ledger = persisted_ledger(entries, updated_at_ms);
        let text = serde_json::to_string_pretty(&ledger).map_err(std::io::Error::other)?;
        let tmp_path = tmp_path_for(path, updated_at_ms);
        std::fs::write(&tmp_path, text)?;
        match std::fs::rename(&tmp_path, path) {
            Ok(()) => Ok(()),
            Err(err) if should_retry_ledger_rename_after_remove(err.kind()) => {
                let _ = std::fs::remove_file(path);
                match std::fs::rename(&tmp_path, path) {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        let _ = std::fs::remove_file(&tmp_path);
                        Err(err)
                    }
                }
            }
            Err(err) => {
                let _ = std::fs::remove_file(&tmp_path);
                Err(err)
            }
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
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let ledger = persisted_ledger(entries, updated_at_ms);
        let text = serde_json::to_string_pretty(&ledger).map_err(std::io::Error::other)?;
        let tmp_path = tmp_path_for(path, updated_at_ms);
        tokio::fs::write(&tmp_path, text).await?;
        match tokio::fs::rename(&tmp_path, path).await {
            Ok(()) => Ok(()),
            Err(err) if should_retry_ledger_rename_after_remove(err.kind()) => {
                let _ = tokio::fs::remove_file(path).await;
                match tokio::fs::rename(&tmp_path, path).await {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        Err(err)
                    }
                }
            }
            Err(err) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                Err(err)
            }
        }
    }
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

fn tmp_path_for(path: &Path, updated_at_ms: u64) -> PathBuf {
    path.with_extension(format!(
        "json.tmp-{}-{}-{}",
        std::process::id(),
        updated_at_ms,
        uuid::Uuid::new_v4()
    ))
}

fn should_retry_ledger_rename_after_remove(kind: std::io::ErrorKind) -> bool {
    kind == std::io::ErrorKind::AlreadyExists
        || (cfg!(windows) && kind == std::io::ErrorKind::PermissionDenied)
}
