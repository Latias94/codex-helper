use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::session_identity::SessionRouteAffinity;

const SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION: u32 = 1;
const SESSION_ROUTE_AFFINITY_LEDGER_ENV: &str = "CODEX_HELPER_SESSION_ROUTE_AFFINITY_LEDGER";

#[derive(Debug, Clone)]
pub(super) struct SessionRouteAffinityStore {
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSessionRouteAffinityLedger {
    schema_version: u32,
    updated_at_ms: u64,
    #[serde(default)]
    entries: HashMap<String, SessionRouteAffinity>,
}

impl SessionRouteAffinityStore {
    pub(super) fn from_env() -> Self {
        if let Ok(value) = std::env::var(SESSION_ROUTE_AFFINITY_LEDGER_ENV) {
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
                    .join("session-route-affinities.json"),
            ),
        }
    }

    pub(super) fn load(&self) -> HashMap<String, SessionRouteAffinity> {
        let Some(path) = self.path.as_ref() else {
            return HashMap::new();
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
            Err(err) => {
                warn!(path = %path.display(), error = %err, "failed to read session route affinity ledger");
                return HashMap::new();
            }
        };
        let ledger = match serde_json::from_str::<PersistedSessionRouteAffinityLedger>(&text) {
            Ok(ledger) => ledger,
            Err(err) => {
                warn!(path = %path.display(), error = %err, "failed to parse session route affinity ledger");
                return HashMap::new();
            }
        };
        if ledger.schema_version != SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION {
            warn!(
                path = %path.display(),
                schema_version = ledger.schema_version,
                supported_schema_version = SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION,
                "ignoring unsupported session route affinity ledger schema"
            );
            return HashMap::new();
        }
        ledger.entries
    }

    pub(super) async fn save(
        &self,
        entries: &HashMap<String, SessionRouteAffinity>,
        updated_at_ms: u64,
    ) -> std::io::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let ledger = PersistedSessionRouteAffinityLedger {
            schema_version: SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION,
            updated_at_ms,
            entries: entries.clone(),
        };
        let text = serde_json::to_string_pretty(&ledger).map_err(std::io::Error::other)?;
        let tmp_path = path.with_extension(format!(
            "json.tmp-{}-{}-{}",
            std::process::id(),
            updated_at_ms,
            uuid::Uuid::new_v4()
        ));
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

fn should_retry_ledger_rename_after_remove(kind: std::io::ErrorKind) -> bool {
    kind == std::io::ErrorKind::AlreadyExists
        || (cfg!(windows) && kind == std::io::ErrorKind::PermissionDenied)
}
