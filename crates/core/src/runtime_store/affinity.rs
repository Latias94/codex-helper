use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::runtime_identity::ProviderEndpointKey;

use super::{RuntimeStoreError, invalid_metadata, sqlite_error};

pub(super) const SESSION_ROUTE_AFFINITIES_SQL: &str = "CREATE TABLE session_route_affinities (store_id TEXT NOT NULL, session_id TEXT NOT NULL CHECK (typeof(session_id) = 'text' AND length(session_id) > 0), route_graph_key TEXT NOT NULL CHECK (typeof(route_graph_key) = 'text' AND length(route_graph_key) > 0), session_identity_source TEXT CHECK (session_identity_source IS NULL OR session_identity_source IN ('header', 'body_session_id', 'prompt_cache_key', 'metadata_session_id', 'previous_response_id')), provider_service_name TEXT NOT NULL CHECK (typeof(provider_service_name) = 'text' AND length(provider_service_name) > 0), provider_id TEXT NOT NULL CHECK (typeof(provider_id) = 'text' AND length(provider_id) > 0), endpoint_id TEXT NOT NULL CHECK (typeof(endpoint_id) = 'text' AND length(endpoint_id) > 0), upstream_base_url TEXT NOT NULL CHECK (typeof(upstream_base_url) = 'text' AND length(upstream_base_url) > 0), route_path_json TEXT NOT NULL CHECK (typeof(route_path_json) = 'text' AND json_valid(route_path_json)), last_selected_at_unix_ms INTEGER NOT NULL CHECK (last_selected_at_unix_ms >= 0), last_changed_at_unix_ms INTEGER NOT NULL CHECK (last_changed_at_unix_ms >= 0 AND last_changed_at_unix_ms <= last_selected_at_unix_ms), change_reason TEXT NOT NULL CHECK (typeof(change_reason) = 'text' AND length(change_reason) > 0), PRIMARY KEY (store_id, session_id), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const SESSION_ROUTE_AFFINITIES_LRU_SQL: &str = "CREATE INDEX session_route_affinities_lru ON session_route_affinities(store_id, last_selected_at_unix_ms, session_id)";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionAffinityIdentitySource {
    Header,
    BodySessionId,
    PromptCacheKey,
    MetadataSessionId,
    PreviousResponseId,
}

impl SessionAffinityIdentitySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::BodySessionId => "body_session_id",
            Self::PromptCacheKey => "prompt_cache_key",
            Self::MetadataSessionId => "metadata_session_id",
            Self::PreviousResponseId => "previous_response_id",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "header" => Ok(Self::Header),
            "body_session_id" => Ok(Self::BodySessionId),
            "prompt_cache_key" => Ok(Self::PromptCacheKey),
            "metadata_session_id" => Ok(Self::MetadataSessionId),
            "previous_response_id" => Ok(Self::PreviousResponseId),
            other => Err(invalid_metadata(
                path,
                format!("session affinity has invalid identity source {other:?}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionAffinityRecord {
    pub session_id: String,
    pub route_graph_key: String,
    pub session_identity_source: Option<SessionAffinityIdentitySource>,
    pub provider_endpoint: ProviderEndpointKey,
    pub upstream_base_url: String,
    pub route_path: Vec<String>,
    pub last_selected_at_unix_ms: u64,
    pub last_changed_at_unix_ms: u64,
    pub change_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAffinityLimit {
    Unlimited,
    MaxEntries(usize),
}

pub(super) fn upsert_session_affinity(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    record: &SessionAffinityRecord,
    limit: SessionAffinityLimit,
) -> Result<(), RuntimeStoreError> {
    validate_record(record)?;
    let selected_at = checked_integer(
        record.last_selected_at_unix_ms,
        &record.session_id,
        "last_selected_at_unix_ms",
    )?;
    let changed_at = checked_integer(
        record.last_changed_at_unix_ms,
        &record.session_id,
        "last_changed_at_unix_ms",
    )?;
    let route_path_json = serde_json::to_string(&record.route_path).map_err(|source| {
        RuntimeStoreError::InvariantViolation {
            entity: "session affinity",
            id: record.session_id.clone(),
            detail: format!("route_path cannot be serialized: {source}"),
        }
    })?;
    transaction
        .execute(
            "INSERT INTO session_route_affinities (
                store_id, session_id, route_graph_key, session_identity_source,
                provider_service_name, provider_id, endpoint_id, upstream_base_url,
                route_path_json, last_selected_at_unix_ms, last_changed_at_unix_ms,
                change_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(store_id, session_id) DO UPDATE SET
                route_graph_key = excluded.route_graph_key,
                session_identity_source = excluded.session_identity_source,
                provider_service_name = excluded.provider_service_name,
                provider_id = excluded.provider_id,
                endpoint_id = excluded.endpoint_id,
                upstream_base_url = excluded.upstream_base_url,
                route_path_json = excluded.route_path_json,
                last_selected_at_unix_ms = excluded.last_selected_at_unix_ms,
                last_changed_at_unix_ms = excluded.last_changed_at_unix_ms,
                change_reason = excluded.change_reason",
            params![
                store_id.to_string(),
                record.session_id,
                record.route_graph_key,
                record
                    .session_identity_source
                    .map(SessionAffinityIdentitySource::as_str),
                record.provider_endpoint.service_name,
                record.provider_endpoint.provider_id,
                record.provider_endpoint.endpoint_id,
                record.upstream_base_url,
                route_path_json,
                selected_at,
                changed_at,
                record.change_reason,
            ],
        )
        .map_err(|source| sqlite_error(path, "upsert session affinity", source))?;
    enforce_limit(transaction, path, store_id, limit)?;
    Ok(())
}

pub(super) fn delete_session_affinity(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    session_id: &str,
) -> Result<bool, RuntimeStoreError> {
    if session_id.trim().is_empty() {
        return Err(RuntimeStoreError::InvariantViolation {
            entity: "session affinity",
            id: session_id.to_string(),
            detail: "session_id is empty".to_string(),
        });
    }
    transaction
        .execute(
            "DELETE FROM session_route_affinities WHERE store_id = ?1 AND session_id = ?2",
            params![store_id.to_string(), session_id],
        )
        .map(|removed| removed > 0)
        .map_err(|source| sqlite_error(path, "delete session affinity", source))
}

pub(super) fn get_session_affinity(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    session_id: &str,
    now_unix_ms: u64,
    ttl_ms: u64,
) -> Result<Option<SessionAffinityRecord>, RuntimeStoreError> {
    let now = checked_integer(now_unix_ms, session_id, "now_unix_ms")?;
    let ttl = checked_integer(ttl_ms, session_id, "ttl_ms")?;
    connection
        .query_row(
            "SELECT session_id, route_graph_key, session_identity_source,
                    provider_service_name, provider_id, endpoint_id, upstream_base_url,
                    route_path_json, last_selected_at_unix_ms, last_changed_at_unix_ms,
                    change_reason
             FROM session_route_affinities
             WHERE store_id = ?1 AND session_id = ?2
               AND (?4 = 0 OR ?3 - last_selected_at_unix_ms < ?4)",
            params![store_id.to_string(), session_id, now, ttl],
            decode_record_row,
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read session affinity", source))?
        .map(|row| decode_record(path, row))
        .transpose()
}

pub(super) fn list_session_affinities(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    now_unix_ms: u64,
    ttl_ms: u64,
) -> Result<Vec<SessionAffinityRecord>, RuntimeStoreError> {
    let now = checked_integer(now_unix_ms, "list", "now_unix_ms")?;
    let ttl = checked_integer(ttl_ms, "list", "ttl_ms")?;
    let mut statement = connection
        .prepare(
            "SELECT session_id, route_graph_key, session_identity_source,
                    provider_service_name, provider_id, endpoint_id, upstream_base_url,
                    route_path_json, last_selected_at_unix_ms, last_changed_at_unix_ms,
                    change_reason
             FROM session_route_affinities
             WHERE store_id = ?1
               AND (?3 = 0 OR ?2 - last_selected_at_unix_ms < ?3)
             ORDER BY last_selected_at_unix_ms ASC, session_id ASC",
        )
        .map_err(|source| sqlite_error(path, "prepare session affinity list", source))?;
    let rows = statement
        .query_map(params![store_id.to_string(), now, ttl], decode_record_row)
        .map_err(|source| sqlite_error(path, "read session affinity list", source))?;
    rows.map(|row| {
        row.map_err(|source| sqlite_error(path, "decode session affinity row", source))
            .and_then(|row| decode_record(path, row))
    })
    .collect()
}

pub(super) fn count_session_affinities(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
) -> Result<u64, RuntimeStoreError> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM session_route_affinities WHERE store_id = ?1",
            [store_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| sqlite_error(path, "count session affinities", source))?;
    u64::try_from(count).map_err(|_| invalid_metadata(path, "session affinity count is negative"))
}

pub(super) fn prune_session_affinities(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    now_unix_ms: u64,
    ttl_ms: u64,
    limit: SessionAffinityLimit,
) -> Result<u64, RuntimeStoreError> {
    let now = checked_integer(now_unix_ms, "prune", "now_unix_ms")?;
    let ttl = checked_integer(ttl_ms, "prune", "ttl_ms")?;
    let store_id_text = store_id.to_string();
    let mut removed = if ttl == 0 {
        0
    } else {
        transaction
            .execute(
                "DELETE FROM session_route_affinities
                 WHERE store_id = ?1 AND ?2 - last_selected_at_unix_ms >= ?3",
                params![store_id_text, now, ttl],
            )
            .map_err(|source| sqlite_error(path, "prune expired session affinities", source))?
    };
    removed = removed.saturating_add(enforce_limit(transaction, path, store_id, limit)?);
    u64::try_from(removed).map_err(|_| RuntimeStoreError::InvariantViolation {
        entity: "session affinity",
        id: "prune".to_string(),
        detail: "removed row count exceeds u64".to_string(),
    })
}

fn enforce_limit(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    limit: SessionAffinityLimit,
) -> Result<usize, RuntimeStoreError> {
    let SessionAffinityLimit::MaxEntries(limit) = limit else {
        return Ok(0);
    };
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    transaction
        .execute(
            "DELETE FROM session_route_affinities
             WHERE store_id = ?1 AND session_id IN (
                SELECT session_id FROM session_route_affinities
                WHERE store_id = ?1
                ORDER BY last_selected_at_unix_ms ASC, session_id ASC
                LIMIT MAX(0, (
                    SELECT COUNT(*) FROM session_route_affinities WHERE store_id = ?1
                ) - ?2)
             )",
            params![store_id.to_string(), limit],
        )
        .map_err(|source| sqlite_error(path, "enforce session affinity capacity", source))
}

type RecordRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    String,
);

fn decode_record_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn decode_record(path: &Path, row: RecordRow) -> Result<SessionAffinityRecord, RuntimeStoreError> {
    let (
        session_id,
        route_graph_key,
        identity_source,
        service_name,
        provider_id,
        endpoint_id,
        upstream_base_url,
        route_path_json,
        selected_at,
        changed_at,
        change_reason,
    ) = row;
    let route_path = serde_json::from_str::<Vec<String>>(&route_path_json).map_err(|source| {
        invalid_metadata(path, format!("invalid affinity route_path: {source}"))
    })?;
    let record = SessionAffinityRecord {
        session_id,
        route_graph_key,
        session_identity_source: identity_source
            .as_deref()
            .map(|value| SessionAffinityIdentitySource::parse(path, value))
            .transpose()?,
        provider_endpoint: ProviderEndpointKey::new(service_name, provider_id, endpoint_id),
        upstream_base_url,
        route_path,
        last_selected_at_unix_ms: decode_nonnegative(path, selected_at, "last_selected_at")?,
        last_changed_at_unix_ms: decode_nonnegative(path, changed_at, "last_changed_at")?,
        change_reason,
    };
    validate_record(&record).map_err(|error| {
        invalid_metadata(path, format!("invalid persisted session affinity: {error}"))
    })?;
    Ok(record)
}

fn validate_record(record: &SessionAffinityRecord) -> Result<(), RuntimeStoreError> {
    let required = [
        ("session_id", record.session_id.as_str()),
        ("route_graph_key", record.route_graph_key.as_str()),
        (
            "provider service",
            record.provider_endpoint.service_name.as_str(),
        ),
        ("provider_id", record.provider_endpoint.provider_id.as_str()),
        ("endpoint_id", record.provider_endpoint.endpoint_id.as_str()),
        ("upstream_base_url", record.upstream_base_url.as_str()),
        ("change_reason", record.change_reason.as_str()),
    ];
    if let Some((field, _)) = required.iter().find(|(_, value)| value.trim().is_empty()) {
        return Err(RuntimeStoreError::InvariantViolation {
            entity: "session affinity",
            id: record.session_id.clone(),
            detail: format!("{field} is empty"),
        });
    }
    if record.last_changed_at_unix_ms > record.last_selected_at_unix_ms {
        return Err(RuntimeStoreError::InvariantViolation {
            entity: "session affinity",
            id: record.session_id.clone(),
            detail: "last_changed_at_unix_ms exceeds last_selected_at_unix_ms".to_string(),
        });
    }
    Ok(())
}

fn checked_integer(value: u64, id: &str, field: &str) -> Result<i64, RuntimeStoreError> {
    i64::try_from(value).map_err(|_| RuntimeStoreError::InvariantViolation {
        entity: "session affinity",
        id: id.to_string(),
        detail: format!("{field} exceeds SQLite integer range"),
    })
}

fn decode_nonnegative(path: &Path, value: i64, field: &str) -> Result<u64, RuntimeStoreError> {
    u64::try_from(value).map_err(|_| invalid_metadata(path, format!("{field} is negative")))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::runtime_store::{RuntimeStore, RuntimeStoreReader};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-helper-runtime-store-{label}-{}-{}",
                std::process::id(),
                Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("create temp directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn affinity(session_id: &str, last_selected_at_unix_ms: u64) -> SessionAffinityRecord {
        SessionAffinityRecord {
            session_id: session_id.to_string(),
            route_graph_key: "codex/main".to_string(),
            session_identity_source: Some(SessionAffinityIdentitySource::Header),
            provider_endpoint: ProviderEndpointKey::new("codex", "primary", "default"),
            upstream_base_url: "https://api.example.test/v1".to_string(),
            route_path: vec!["main".to_string(), "primary".to_string()],
            last_selected_at_unix_ms,
            last_changed_at_unix_ms: last_selected_at_unix_ms,
            change_reason: "first_success".to_string(),
        }
    }

    #[test]
    fn session_affinity_round_trips_across_reopen_and_read_only_reader() {
        let home = TestDir::new("affinity-reopen");
        let store = RuntimeStore::open_in_home(home.path()).expect("open store");
        let expected = affinity("session-a", 100);
        store
            .upsert_session_affinity(expected.clone(), SessionAffinityLimit::Unlimited)
            .expect("persist affinity");
        drop(store);

        let reopened = RuntimeStore::open_in_home(home.path()).expect("reopen store");
        assert_eq!(
            reopened
                .get_session_affinity("session-a", 101, 0)
                .expect("read affinity"),
            Some(expected.clone())
        );
        let reader = RuntimeStoreReader::open_in_home(home.path()).expect("open reader");
        assert_eq!(
            reader
                .get_session_affinity("session-a", 101, 0)
                .expect("read affinity through reader"),
            Some(expected)
        );
    }

    #[test]
    fn deleting_one_affinity_preserves_other_sessions() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        for session_id in ["session-a", "session-b"] {
            store
                .upsert_session_affinity(affinity(session_id, 100), SessionAffinityLimit::Unlimited)
                .expect("persist affinity");
        }

        assert!(
            store
                .delete_session_affinity("session-a")
                .expect("delete affinity")
        );
        assert_eq!(
            store
                .get_session_affinity("session-a", 101, 0)
                .expect("read deleted affinity"),
            None
        );
        assert!(
            store
                .get_session_affinity("session-b", 101, 0)
                .expect("read preserved affinity")
                .is_some()
        );
        assert!(
            !store
                .delete_session_affinity("session-a")
                .expect("repeat delete")
        );
    }

    #[test]
    fn expired_affinity_is_hidden_until_explicit_prune() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        store
            .upsert_session_affinity(affinity("expired", 100), SessionAffinityLimit::Unlimited)
            .expect("persist affinity");

        assert_eq!(
            store
                .get_session_affinity("expired", 200, 100)
                .expect("read expired affinity"),
            None
        );
        assert_eq!(
            store
                .count_session_affinities()
                .expect("count durable rows"),
            1
        );
        assert_eq!(
            store
                .prune_session_affinities(200, 100, SessionAffinityLimit::Unlimited)
                .expect("prune expired affinity"),
            1
        );
        assert_eq!(
            store.count_session_affinities().expect("count pruned rows"),
            0
        );
    }

    #[test]
    fn affinity_limit_evicts_oldest_with_stable_session_tie_break() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let limit = SessionAffinityLimit::MaxEntries(2);
        store
            .upsert_session_affinity(affinity("b", 100), limit)
            .expect("persist b");
        store
            .upsert_session_affinity(affinity("a", 100), limit)
            .expect("persist a");
        store
            .upsert_session_affinity(affinity("c", 200), limit)
            .expect("persist c");

        let sessions = store
            .list_session_affinities(200, 0)
            .expect("list affinities")
            .into_iter()
            .map(|entry| entry.session_id)
            .collect::<Vec<_>>();
        assert_eq!(sessions, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn injected_affinity_failure_preserves_previous_value() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let previous = affinity("session-a", 100);
        store
            .upsert_session_affinity(previous.clone(), SessionAffinityLimit::Unlimited)
            .expect("persist initial affinity");
        let mut replacement = affinity("session-a", 200);
        replacement.provider_endpoint = ProviderEndpointKey::new("codex", "backup", "default");

        store.fail_next_affinity_commit_for_test();
        let error = store
            .upsert_session_affinity(replacement, SessionAffinityLimit::Unlimited)
            .expect_err("injected failure must roll back");
        assert!(matches!(error, RuntimeStoreError::InjectedFailure { .. }));
        assert_eq!(
            store
                .get_session_affinity("session-a", 201, 0)
                .expect("read prior affinity"),
            Some(previous)
        );
    }
}
