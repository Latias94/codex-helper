use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        let ledger = PersistedSessionRouteAffinityLedger {
            schema_version: SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION,
            updated_at_ms,
            entries: entries.clone(),
        };
        let text = serde_json::to_string_pretty(&ledger).map_err(std::io::Error::other)?;
        match crate::file_replace::write_bytes_file_async(path, text.as_bytes()).await {
            Ok(()) => Ok(()),
            Err(err) => recover_session_route_candidate(path, text.as_bytes(), err).await,
        }
    }
}

fn validate_session_route_candidate(bytes: &[u8]) -> std::io::Result<()> {
    let ledger = serde_json::from_slice::<PersistedSessionRouteAffinityLedger>(bytes)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    if ledger.schema_version != SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unsupported session route affinity ledger schema {}; expected {}",
                ledger.schema_version, SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION
            ),
        ));
    }
    Ok(())
}

async fn recover_session_route_candidate(
    path: &Path,
    candidate: &[u8],
    error: crate::file_replace::AtomicWriteError,
) -> std::io::Result<()> {
    crate::file_replace::recover_uncertain_candidate_async(
        path,
        candidate,
        error,
        validate_session_route_candidate,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestFile(PathBuf);

    impl TestFile {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "codex-helper-session-route-store-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&directory).expect("create test directory");
            Self(directory.join("session-routes.json"))
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

    #[tokio::test]
    async fn uncertain_write_recovers_only_the_exact_valid_session_route_candidate() {
        let path = TestFile::new();
        let candidate = serde_json::to_vec_pretty(&PersistedSessionRouteAffinityLedger {
            schema_version: SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION,
            updated_at_ms: 2,
            entries: HashMap::new(),
        })
        .expect("serialize candidate");
        tokio::fs::write(&path.0, &candidate)
            .await
            .expect("write recovered candidate");

        recover_session_route_candidate(&path.0, &candidate, uncertain_error(&path.0))
            .await
            .expect("exact valid candidate should recover");

        let old = serde_json::to_vec_pretty(&PersistedSessionRouteAffinityLedger {
            schema_version: SESSION_ROUTE_AFFINITY_LEDGER_SCHEMA_VERSION,
            updated_at_ms: 1,
            entries: HashMap::new(),
        })
        .expect("serialize old ledger");
        tokio::fs::write(&path.0, &old)
            .await
            .expect("write old ledger");
        let err = recover_session_route_candidate(&path.0, &candidate, uncertain_error(&path.0))
            .await
            .expect_err("different valid bytes must not recover as the candidate");
        assert!(err.to_string().contains("do not match the candidate"));
        assert_eq!(
            tokio::fs::read(&path.0).await.expect("read old ledger"),
            old
        );
    }
}
