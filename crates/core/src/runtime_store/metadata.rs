use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use super::{RuntimeStoreError, invalid_metadata, sqlite_error};

pub(super) const RUNTIME_PRIVATE_KEYS_SQL: &str = "CREATE TABLE runtime_private_keys (store_id TEXT NOT NULL, key_name TEXT NOT NULL CHECK (key_name IN ('quota_identity')), revision INTEGER NOT NULL CHECK (revision >= 1), key_material BLOB NOT NULL CHECK (typeof(key_material) = 'blob' AND length(key_material) = 32 AND key_material <> zeroblob(32)), created_at_unix_ms INTEGER NOT NULL CHECK (created_at_unix_ms >= 0), PRIMARY KEY (store_id, key_name), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const RUNTIME_DOCUMENTS_SQL: &str = "CREATE TABLE runtime_documents (store_id TEXT NOT NULL, document_kind TEXT NOT NULL CHECK (document_kind IN ('quota_registry', 'basellm_catalog')), schema_version INTEGER NOT NULL CHECK (schema_version >= 1), revision INTEGER NOT NULL CHECK (revision >= 1), updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= 0), payload_json TEXT NOT NULL CHECK (typeof(payload_json) = 'text' AND length(payload_json) > 0 AND json_valid(payload_json)), PRIMARY KEY (store_id, document_kind), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";

pub(super) fn migration_sql() -> String {
    format!(
        "{};\n{};\n",
        RUNTIME_PRIVATE_KEYS_SQL, RUNTIME_DOCUMENTS_SQL
    )
}

/// Installation-local secret used to derive credential-safe quota pool identities.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeQuotaIdentity {
    key: [u8; 32],
    revision: u64,
}

impl RuntimeQuotaIdentity {
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

impl fmt::Debug for RuntimeQuotaIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeQuotaIdentity")
            .field("key", &"<redacted>")
            .field("revision", &self.revision)
            .finish()
    }
}

/// Versioned helper-owned document stored inside the canonical runtime database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeDocumentKind {
    QuotaRegistry,
    BasellmCatalog,
}

impl RuntimeDocumentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::QuotaRegistry => "quota_registry",
            Self::BasellmCatalog => "basellm_catalog",
        }
    }
}

/// One committed runtime document revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDocument {
    pub kind: RuntimeDocumentKind,
    pub schema_version: u32,
    pub revision: u64,
    pub updated_at_unix_ms: u64,
    pub payload_json: String,
}

/// Candidate document content written atomically with the other candidates in a batch.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeDocumentWrite<'a> {
    pub kind: RuntimeDocumentKind,
    pub schema_version: u32,
    pub payload_json: &'a str,
}

/// Result of conditionally committing one runtime document revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDocumentCommit {
    Committed(RuntimeDocument),
    Stale(Option<RuntimeDocument>),
}

pub(super) fn load_or_create_quota_identity(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    created_at_unix_ms: u64,
) -> Result<RuntimeQuotaIdentity, RuntimeStoreError> {
    if let Some(identity) = read_quota_identity(transaction, path, store_id)? {
        return Ok(identity);
    }

    let first = *Uuid::new_v4().as_bytes();
    let second = *Uuid::new_v4().as_bytes();
    let mut key = [0_u8; 32];
    key[..16].copy_from_slice(&first);
    key[16..].copy_from_slice(&second);
    let created_at =
        i64::try_from(created_at_unix_ms).map_err(|_| RuntimeStoreError::InvariantViolation {
            entity: "runtime private key",
            id: "quota_identity".to_string(),
            detail: "created_at_unix_ms exceeds SQLite integer range".to_string(),
        })?;
    transaction
        .execute(
            "INSERT INTO runtime_private_keys (
                store_id, key_name, revision, key_material, created_at_unix_ms
             ) VALUES (?1, 'quota_identity', 1, ?2, ?3)",
            params![store_id.to_string(), key.as_slice(), created_at],
        )
        .map_err(|source| sqlite_error(path, "create quota identity key", source))?;
    Ok(RuntimeQuotaIdentity { key, revision: 1 })
}

pub(super) fn read_quota_identity(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
) -> Result<Option<RuntimeQuotaIdentity>, RuntimeStoreError> {
    let row = connection
        .query_row(
            "SELECT revision, key_material
             FROM runtime_private_keys
             WHERE store_id = ?1 AND key_name = 'quota_identity'",
            params![store_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read quota identity key", source))?;
    row.map(|(revision, material)| decode_quota_identity(path, revision, material))
        .transpose()
}

pub(super) fn read_document(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    kind: RuntimeDocumentKind,
) -> Result<Option<RuntimeDocument>, RuntimeStoreError> {
    let row = connection
        .query_row(
            "SELECT schema_version, revision, updated_at_unix_ms, payload_json
             FROM runtime_documents
             WHERE store_id = ?1 AND document_kind = ?2",
            params![store_id.to_string(), kind.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read runtime document", source))?;
    row.map(
        |(schema_version, revision, updated_at_unix_ms, payload_json)| {
            Ok(RuntimeDocument {
                kind,
                schema_version: decode_positive_u32(
                    path,
                    schema_version,
                    "document schema version",
                )?,
                revision: decode_positive_u64(path, revision, "document revision")?,
                updated_at_unix_ms: decode_nonnegative_u64(
                    path,
                    updated_at_unix_ms,
                    "document update timestamp",
                )?,
                payload_json,
            })
        },
    )
    .transpose()
}

pub(super) fn write_documents(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    updated_at_unix_ms: u64,
    writes: &[RuntimeDocumentWrite<'_>],
) -> Result<Vec<RuntimeDocument>, RuntimeStoreError> {
    if writes.is_empty() {
        return Ok(Vec::new());
    }
    let updated_at =
        i64::try_from(updated_at_unix_ms).map_err(|_| RuntimeStoreError::InvariantViolation {
            entity: "runtime document",
            id: "batch".to_string(),
            detail: "updated_at_unix_ms exceeds SQLite integer range".to_string(),
        })?;
    for (index, write) in writes.iter().enumerate() {
        if write.schema_version == 0 {
            return Err(RuntimeStoreError::InvariantViolation {
                entity: "runtime document",
                id: write.kind.as_str().to_string(),
                detail: "schema_version must be positive".to_string(),
            });
        }
        if writes[..index]
            .iter()
            .any(|candidate| candidate.kind == write.kind)
        {
            return Err(RuntimeStoreError::InvariantViolation {
                entity: "runtime document",
                id: write.kind.as_str().to_string(),
                detail: "document kind appears more than once in one batch".to_string(),
            });
        }
        if serde_json::from_str::<serde_json::Value>(write.payload_json).is_err() {
            return Err(RuntimeStoreError::InvariantViolation {
                entity: "runtime document",
                id: write.kind.as_str().to_string(),
                detail: "payload_json is not valid JSON".to_string(),
            });
        }
        transaction
            .execute(
                "INSERT INTO runtime_documents (
                    store_id, document_kind, schema_version, revision,
                    updated_at_unix_ms, payload_json
                 ) VALUES (?1, ?2, ?3, 1, ?4, ?5)
                 ON CONFLICT (store_id, document_kind) DO UPDATE SET
                    schema_version = excluded.schema_version,
                    revision = runtime_documents.revision + 1,
                    updated_at_unix_ms = excluded.updated_at_unix_ms,
                    payload_json = excluded.payload_json",
                params![
                    store_id.to_string(),
                    write.kind.as_str(),
                    i64::from(write.schema_version),
                    updated_at,
                    write.payload_json,
                ],
            )
            .map_err(|source| sqlite_error(path, "write runtime document", source))?;
    }
    writes
        .iter()
        .map(|write| {
            read_document(transaction, path, store_id, write.kind)?.ok_or_else(|| {
                invalid_metadata(
                    path,
                    format!(
                        "runtime document {:?} disappeared inside its commit transaction",
                        write.kind
                    ),
                )
            })
        })
        .collect()
}

fn decode_quota_identity(
    path: &Path,
    revision: i64,
    material: Vec<u8>,
) -> Result<RuntimeQuotaIdentity, RuntimeStoreError> {
    let revision = decode_positive_u64(path, revision, "quota identity revision")?;
    let key: [u8; 32] = material
        .try_into()
        .map_err(|_| invalid_metadata(path, "quota identity key is not a 32-byte secret"))?;
    if key.iter().all(|byte| *byte == 0) {
        return Err(invalid_metadata(path, "quota identity key is empty"));
    }
    Ok(RuntimeQuotaIdentity { key, revision })
}

fn decode_positive_u32(
    path: &Path,
    value: i64,
    field: &'static str,
) -> Result<u32, RuntimeStoreError> {
    let value = u32::try_from(value)
        .map_err(|_| invalid_metadata(path, format!("{field} is outside u32 range")))?;
    if value == 0 {
        Err(invalid_metadata(path, format!("{field} must be positive")))
    } else {
        Ok(value)
    }
}

fn decode_positive_u64(
    path: &Path,
    value: i64,
    field: &'static str,
) -> Result<u64, RuntimeStoreError> {
    let value = decode_nonnegative_u64(path, value, field)?;
    if value == 0 {
        Err(invalid_metadata(path, format!("{field} must be positive")))
    } else {
        Ok(value)
    }
}

fn decode_nonnegative_u64(
    path: &Path,
    value: i64,
    field: &'static str,
) -> Result<u64, RuntimeStoreError> {
    u64::try_from(value).map_err(|_| invalid_metadata(path, format!("{field} is negative")))
}
