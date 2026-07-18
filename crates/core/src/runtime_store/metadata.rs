use std::fmt;
use std::path::Path;

use base64::Engine as _;
use hmac::{Hmac, Mac as _};
use http::HeaderMap;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::Sha256;
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

    pub(crate) fn derive_credential_scope(
        &self,
        bearer: Option<&[u8]>,
        api_key: Option<&[u8]>,
    ) -> Option<String> {
        if bearer.is_none() && api_key.is_none() {
            return None;
        }
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .expect("runtime quota identity is a valid HMAC key");
        mac.update(b"codex-helper/runtime-credential-scope/hmac-sha256/v1\0");
        for value in [bearer, api_key] {
            match value {
                Some(value) => {
                    mac.update(&[1]);
                    mac.update(&(value.len() as u64).to_be_bytes());
                    mac.update(value);
                }
                None => mac.update(&[0]),
            }
        }
        let digest = mac.finalize().into_bytes();
        let opaque = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Some(format!("hmac-sha256-v1:{opaque}"))
    }

    pub(crate) fn derive_usage_account_fingerprint(
        &self,
        token: &[u8],
        new_api_user_id: Option<&str>,
    ) -> String {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .expect("runtime quota identity is a valid HMAC key");
        mac.update(b"codex-helper/usage-provider-account/hmac-sha256/v1\0");
        mac.update(&(token.len() as u64).to_be_bytes());
        mac.update(token);
        match new_api_user_id {
            Some(user_id) => {
                mac.update(&[1]);
                mac.update(&(user_id.len() as u64).to_be_bytes());
                mac.update(user_id.as_bytes());
            }
            None => mac.update(&[0]),
        }
        format!("sha256:{:x}", mac.finalize().into_bytes())
    }

    pub(crate) fn derive_provider_account_fingerprint(
        &self,
        final_headers: &HeaderMap,
    ) -> Option<[u8; 32]> {
        const ACCOUNT_HEADERS: &[&str] = &[
            "authorization",
            "chatgpt-account-id",
            "openai-organization",
            "openai-project",
            "x-api-key",
            "x-openai-actor-authorization",
            "x-openai-fedramp",
            "x-openai-organization",
            "x-openai-project",
            "x-organization-id",
            "x-project-id",
        ];

        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(&self.key)
            .expect("runtime quota identity is a valid HMAC key");
        mac.update(b"codex-helper/provider-account/hmac-sha256/v1\0");
        let mut found = false;
        for name in ACCOUNT_HEADERS {
            let mut values = final_headers
                .get_all(*name)
                .iter()
                .map(|value| normalized_provider_account_header(name, value.as_bytes()))
                .collect::<Vec<_>>();
            values.sort();
            for value in values {
                found = true;
                mac.update(&(name.len() as u64).to_be_bytes());
                mac.update(name.as_bytes());
                mac.update(&(value.len() as u64).to_be_bytes());
                mac.update(&value);
            }
        }
        found.then(|| mac.finalize().into_bytes().into())
    }
}

fn normalized_provider_account_header(name: &str, value: &[u8]) -> Vec<u8> {
    if name != "authorization" {
        return value.to_vec();
    }
    let Ok(value) = std::str::from_utf8(value) else {
        return value.to_vec();
    };
    let Some(fields) = value.strip_prefix("AWS4-HMAC-SHA256") else {
        return value.as_bytes().to_vec();
    };
    fields
        .trim_start()
        .split(',')
        .find_map(|part| part.trim().strip_prefix("Credential="))
        .and_then(|credential| credential.split('/').next())
        .filter(|credential| !credential.is_empty())
        .map_or_else(
            || value.as_bytes().to_vec(),
            |credential| credential.as_bytes().to_vec(),
        )
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

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue};
    use sha2::{Digest as _, Sha256};

    use super::RuntimeQuotaIdentity;

    fn identity(key: [u8; 32]) -> RuntimeQuotaIdentity {
        RuntimeQuotaIdentity { key, revision: 1 }
    }

    fn provider_headers(token: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static(token));
        headers
    }

    #[test]
    fn usage_account_fingerprint_is_stable_opaque_and_well_formed() {
        let identity = identity([0x11; 32]);
        let token = b"usage-provider-token";

        let first = identity.derive_usage_account_fingerprint(token, Some("user-42"));
        let second = identity.derive_usage_account_fingerprint(token, Some("user-42"));

        assert_eq!(first, second);
        let digest = first
            .strip_prefix("sha256:")
            .expect("fingerprint uses the persisted account fingerprint prefix");
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(digest, digest.to_ascii_lowercase());
        assert_ne!(first, format!("sha256:{:x}", Sha256::digest(token)));
    }

    #[test]
    fn usage_account_fingerprint_separates_installation_token_and_user() {
        let first_installation = identity([0x11; 32]);
        let second_installation = identity([0x22; 32]);
        let baseline =
            first_installation.derive_usage_account_fingerprint(b"token-a", Some("user-a"));

        assert_ne!(
            baseline,
            second_installation.derive_usage_account_fingerprint(b"token-a", Some("user-a"))
        );
        assert_ne!(
            baseline,
            first_installation.derive_usage_account_fingerprint(b"token-b", Some("user-a"))
        );
        assert_ne!(
            baseline,
            first_installation.derive_usage_account_fingerprint(b"token-a", Some("user-b"))
        );
        assert_ne!(
            baseline,
            first_installation.derive_usage_account_fingerprint(b"token-a", None)
        );
        assert_ne!(
            first_installation.derive_usage_account_fingerprint(b"token-a", Some("")),
            first_installation.derive_usage_account_fingerprint(b"token-a", None)
        );
    }

    #[test]
    fn usage_account_fingerprint_and_identity_debug_do_not_expose_secrets() {
        let key = *b"runtime-key-canary-0123456789abc";
        let token = b"canary-bearer-token-never-log";
        let identity = identity(key);

        let fingerprint = identity.derive_usage_account_fingerprint(token, Some("canary-user"));
        let debug = format!("{identity:?}");

        assert!(!fingerprint.contains("canary-bearer-token-never-log"));
        assert!(!fingerprint.contains("canary-user"));
        assert!(!debug.contains("runtime-key-canary-0123456789abc"));
        assert!(!debug.contains("canary-bearer-token-never-log"));
    }

    #[test]
    fn provider_account_fingerprint_is_stable_and_installation_keyed() {
        let first_installation = identity([0x11; 32]);
        let second_installation = identity([0x22; 32]);
        let account = provider_headers("Bearer provider-account-canary");

        let first = first_installation
            .derive_provider_account_fingerprint(&account)
            .expect("account-bearing headers");
        let same = first_installation
            .derive_provider_account_fingerprint(&account)
            .expect("same account-bearing headers");
        let other_installation = second_installation
            .derive_provider_account_fingerprint(&account)
            .expect("same account on another installation");

        assert_eq!(first, same);
        assert_ne!(first, other_installation);
    }

    #[test]
    fn provider_account_fingerprint_partitions_accounts_and_ignores_request_headers() {
        let identity = identity([0x11; 32]);
        let first = provider_headers("Bearer provider-account-one");
        let mut same_account = first.clone();
        same_account.insert("x-request-id", HeaderValue::from_static("request-two"));
        let second = provider_headers("Bearer provider-account-two");

        assert_eq!(
            identity.derive_provider_account_fingerprint(&first),
            identity.derive_provider_account_fingerprint(&same_account)
        );
        assert_ne!(
            identity.derive_provider_account_fingerprint(&first),
            identity.derive_provider_account_fingerprint(&second)
        );
        assert_eq!(
            identity.derive_provider_account_fingerprint(&HeaderMap::new()),
            None
        );
    }

    #[test]
    fn provider_account_fingerprint_partitions_openai_actor_authorizations() {
        let identity = identity([0x11; 32]);
        let mut first = HeaderMap::new();
        first.insert(
            "x-openai-actor-authorization",
            HeaderValue::from_static("actor-account-one"),
        );
        let mut second = HeaderMap::new();
        second.insert(
            "x-openai-actor-authorization",
            HeaderValue::from_static("actor-account-two"),
        );

        assert_ne!(
            identity.derive_provider_account_fingerprint(&first),
            identity.derive_provider_account_fingerprint(&second)
        );
        assert!(
            identity
                .derive_provider_account_fingerprint(&first)
                .is_some()
        );
    }

    #[test]
    fn provider_account_fingerprint_normalizes_aws_sigv4_to_access_key_identity() {
        let identity = identity([0x11; 32]);
        let first = provider_headers(
            "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260711/us-east-1/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=aaaaaaaa",
        );
        let same_account = provider_headers(
            "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260712/us-west-2/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=bbbbbbbb",
        );
        let other_account = provider_headers(
            "AWS4-HMAC-SHA256 Credential=AKIAOTHER/20260711/us-east-1/bedrock/aws4_request, SignedHeaders=content-type;host;x-amz-date, Signature=aaaaaaaa",
        );

        assert_eq!(
            identity.derive_provider_account_fingerprint(&first),
            identity.derive_provider_account_fingerprint(&same_account)
        );
        assert_ne!(
            identity.derive_provider_account_fingerprint(&first),
            identity.derive_provider_account_fingerprint(&other_account)
        );
    }
}
