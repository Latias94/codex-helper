use std::collections::BTreeMap;
use std::path::Path;

use reqwest::Url;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::runtime_identity::{ProviderEndpointKey, RuntimeUpstreamIdentity};

use super::{RuntimeStoreError, invalid_metadata, sqlite_error};

pub(super) const RUNTIME_REVISIONS_SQL: &str = "CREATE TABLE runtime_revisions (store_id TEXT PRIMARY KEY NOT NULL, policy_revision INTEGER NOT NULL CHECK (policy_revision >= 0), updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= 0), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const RUNTIME_IDENTITY_AUTHORITY_SQL: &str = "CREATE TABLE runtime_identity_authority (store_id TEXT PRIMARY KEY NOT NULL, updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= 0), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const RUNTIME_UPSTREAM_IDENTITIES_SQL: &str = "CREATE TABLE runtime_upstream_identities (store_id TEXT NOT NULL, endpoint_key_json TEXT NOT NULL CHECK (typeof(endpoint_key_json) = 'text' AND json_valid(endpoint_key_json)), route_scope TEXT NOT NULL CHECK (typeof(route_scope) = 'text' AND route_scope LIKE 'sha256:%'), PRIMARY KEY (store_id, endpoint_key_json), FOREIGN KEY (store_id) REFERENCES runtime_identity_authority(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const PROVIDER_INCARNATIONS_SQL: &str = "CREATE TABLE provider_incarnations (store_id TEXT NOT NULL, incarnation_id BLOB NOT NULL CHECK (typeof(incarnation_id) = 'blob' AND length(incarnation_id) = 16 AND incarnation_id <> zeroblob(16)), endpoint_key_json TEXT NOT NULL CHECK (typeof(endpoint_key_json) = 'text' AND json_valid(endpoint_key_json)), scope_digest TEXT NOT NULL CHECK (typeof(scope_digest) = 'text' AND length(scope_digest) > 7), endpoint_origin TEXT NOT NULL CHECK (typeof(endpoint_origin) = 'text' AND length(endpoint_origin) > 0), route_scope TEXT NOT NULL CHECK (typeof(route_scope) = 'text' AND length(route_scope) > 0), adapter_code TEXT NOT NULL CHECK (typeof(adapter_code) = 'text' AND length(adapter_code) > 0), observation_origin TEXT NOT NULL CHECK (typeof(observation_origin) = 'text' AND length(observation_origin) > 0), account_fingerprint TEXT NOT NULL CHECK (typeof(account_fingerprint) = 'text' AND account_fingerprint LIKE 'sha256:%'), config_revision TEXT NOT NULL CHECK (typeof(config_revision) = 'text' AND config_revision LIKE 'sha256:%'), activated_at_unix_ms INTEGER NOT NULL CHECK (activated_at_unix_ms >= 0), deactivated_at_unix_ms INTEGER CHECK (deactivated_at_unix_ms IS NULL OR deactivated_at_unix_ms >= activated_at_unix_ms), last_reserved_generation INTEGER NOT NULL CHECK (last_reserved_generation >= 1), last_accepted_generation INTEGER NOT NULL CHECK (last_accepted_generation >= 0 AND last_accepted_generation <= last_reserved_generation), PRIMARY KEY (store_id, incarnation_id), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const PROVIDER_INCARNATIONS_ENDPOINT_SQL: &str = "CREATE INDEX provider_incarnations_endpoint_history ON provider_incarnations(store_id, endpoint_key_json, activated_at_unix_ms, incarnation_id)";
pub(super) const PROVIDER_ENDPOINT_HEADS_SQL: &str = "CREATE TABLE provider_endpoint_heads (store_id TEXT NOT NULL, endpoint_key_json TEXT NOT NULL CHECK (typeof(endpoint_key_json) = 'text' AND json_valid(endpoint_key_json)), incarnation_id BLOB NOT NULL CHECK (typeof(incarnation_id) = 'blob' AND length(incarnation_id) = 16 AND incarnation_id <> zeroblob(16)), PRIMARY KEY (store_id, endpoint_key_json), UNIQUE (store_id, incarnation_id), FOREIGN KEY (store_id, incarnation_id) REFERENCES provider_incarnations(store_id, incarnation_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const PROVIDER_OBSERVATIONS_SQL: &str = "CREATE TABLE provider_observations (store_id TEXT NOT NULL, endpoint_key_json TEXT NOT NULL CHECK (typeof(endpoint_key_json) = 'text' AND json_valid(endpoint_key_json)), incarnation_id BLOB NOT NULL CHECK (typeof(incarnation_id) = 'blob' AND length(incarnation_id) = 16 AND incarnation_id <> zeroblob(16)), generation INTEGER NOT NULL CHECK (generation >= 1), observed_at_unix_ms INTEGER NOT NULL CHECK (observed_at_unix_ms >= 0), completed_at_unix_ms INTEGER NOT NULL CHECK (completed_at_unix_ms >= observed_at_unix_ms), authority TEXT NOT NULL CHECK (authority IN ('authoritative', 'informational')), evidence_json TEXT NOT NULL CHECK (typeof(evidence_json) = 'text' AND json_valid(evidence_json)), effect_json TEXT NOT NULL CHECK (typeof(effect_json) = 'text' AND json_valid(effect_json)), disposition TEXT NOT NULL CHECK (disposition IN ('accepted', 'ignored_stale', 'ignored_inactive_incarnation')), policy_revision INTEGER NOT NULL CHECK (policy_revision >= 0), PRIMARY KEY (store_id, incarnation_id, generation), FOREIGN KEY (store_id, incarnation_id) REFERENCES provider_incarnations(store_id, incarnation_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";
pub(super) const PROVIDER_OBSERVATIONS_HISTORY_SQL: &str = "CREATE INDEX provider_observations_endpoint_history ON provider_observations(store_id, endpoint_key_json, observed_at_unix_ms, generation)";
pub(super) const PROVIDER_POLICY_ACTIONS_SQL: &str = "CREATE TABLE provider_policy_actions (store_id TEXT NOT NULL, action_id BLOB NOT NULL CHECK (typeof(action_id) = 'blob' AND length(action_id) = 16 AND action_id <> zeroblob(16)), endpoint_key_json TEXT NOT NULL CHECK (typeof(endpoint_key_json) = 'text' AND json_valid(endpoint_key_json)), incarnation_id BLOB NOT NULL CHECK (typeof(incarnation_id) = 'blob' AND length(incarnation_id) = 16 AND incarnation_id <> zeroblob(16)), generation INTEGER NOT NULL CHECK (generation >= 1), action_kind TEXT NOT NULL CHECK (typeof(action_kind) = 'text' AND length(action_kind) > 0), code TEXT, reason TEXT NOT NULL CHECK (typeof(reason) = 'text' AND length(reason) > 0), opened_at_unix_ms INTEGER NOT NULL CHECK (opened_at_unix_ms >= 0), expires_at_unix_ms INTEGER CHECK (expires_at_unix_ms IS NULL OR expires_at_unix_ms >= opened_at_unix_ms), closed_at_unix_ms INTEGER CHECK (closed_at_unix_ms IS NULL OR closed_at_unix_ms >= opened_at_unix_ms), close_reason TEXT, PRIMARY KEY (store_id, action_id), FOREIGN KEY (store_id, incarnation_id) REFERENCES provider_incarnations(store_id, incarnation_id) ON UPDATE RESTRICT ON DELETE RESTRICT, CHECK ((closed_at_unix_ms IS NULL AND close_reason IS NULL) OR (closed_at_unix_ms IS NOT NULL AND typeof(close_reason) = 'text' AND length(close_reason) > 0))) STRICT, WITHOUT ROWID";
pub(super) const PROVIDER_POLICY_ACTIONS_ACTIVE_SQL: &str = "CREATE UNIQUE INDEX provider_policy_actions_active ON provider_policy_actions(store_id, endpoint_key_json) WHERE closed_at_unix_ms IS NULL";
pub(super) const PROVIDER_POLICY_ACTIONS_HISTORY_SQL: &str = "CREATE INDEX provider_policy_actions_history ON provider_policy_actions(store_id, endpoint_key_json, opened_at_unix_ms, action_id)";
pub(super) const PROVIDER_MANUAL_ELIGIBILITY_SQL: &str = "CREATE TABLE provider_manual_eligibility (store_id TEXT NOT NULL, endpoint_key_json TEXT NOT NULL CHECK (typeof(endpoint_key_json) = 'text' AND json_valid(endpoint_key_json)), eligibility TEXT NOT NULL CHECK (eligibility IN ('enabled', 'disabled', 'draining')), reason TEXT, updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= 0), PRIMARY KEY (store_id, endpoint_key_json), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT, CHECK (eligibility = 'enabled' OR (typeof(reason) = 'text' AND length(reason) > 0))) STRICT, WITHOUT ROWID";
pub(super) const PROVIDER_ELIGIBILITY_SQL: &str = "CREATE TABLE provider_eligibility (store_id TEXT NOT NULL, endpoint_key_json TEXT NOT NULL CHECK (typeof(endpoint_key_json) = 'text' AND json_valid(endpoint_key_json)), incarnation_id BLOB CHECK (incarnation_id IS NULL OR (typeof(incarnation_id) = 'blob' AND length(incarnation_id) = 16 AND incarnation_id <> zeroblob(16))), automatic_eligibility TEXT NOT NULL CHECK (automatic_eligibility IN ('eligible', 'blocked')), manual_eligibility TEXT NOT NULL CHECK (manual_eligibility IN ('enabled', 'disabled', 'draining')), effective_eligibility TEXT NOT NULL CHECK (effective_eligibility IN ('eligible', 'ineligible')), active_action_id BLOB CHECK (active_action_id IS NULL OR (typeof(active_action_id) = 'blob' AND length(active_action_id) = 16 AND active_action_id <> zeroblob(16))), manual_reason TEXT, updated_at_unix_ms INTEGER NOT NULL CHECK (updated_at_unix_ms >= 0), policy_revision INTEGER NOT NULL CHECK (policy_revision >= 0), PRIMARY KEY (store_id, endpoint_key_json), FOREIGN KEY (store_id) REFERENCES store_meta(store_id) ON UPDATE RESTRICT ON DELETE RESTRICT, FOREIGN KEY (store_id, incarnation_id) REFERENCES provider_incarnations(store_id, incarnation_id) ON UPDATE RESTRICT ON DELETE RESTRICT, FOREIGN KEY (store_id, active_action_id) REFERENCES provider_policy_actions(store_id, action_id) ON UPDATE RESTRICT ON DELETE RESTRICT) STRICT, WITHOUT ROWID";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderObservationScopeError {
    #[error("provider observation scope field {0} is empty")]
    EmptyField(&'static str),
    #[error("provider observation {0} is not a valid HTTP origin")]
    InvalidOrigin(&'static str),
    #[error("provider observation {0} must not contain credentials")]
    CredentialsNotAllowed(&'static str),
    #[error("provider observation {0} must be a credential-safe sha256 fingerprint")]
    CredentialSafeDigestRequired(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservationScope {
    provider_endpoint: ProviderEndpointKey,
    endpoint_origin: String,
    route_scope: String,
    adapter_code: String,
    observation_origin: String,
    account_fingerprint: String,
    config_revision: String,
}

impl ProviderObservationScope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider_endpoint: ProviderEndpointKey,
        endpoint: impl AsRef<str>,
        route_scope: impl AsRef<str>,
        adapter_code: impl AsRef<str>,
        observation_endpoint: impl AsRef<str>,
        account_fingerprint: impl AsRef<str>,
        config_revision: impl AsRef<str>,
    ) -> Result<Self, ProviderObservationScopeError> {
        validate_endpoint_key(&provider_endpoint)?;
        let endpoint_origin = canonical_origin(endpoint.as_ref(), "endpoint_origin")?;
        let observation_origin =
            canonical_origin(observation_endpoint.as_ref(), "observation_origin")?;
        let route_scope = required(route_scope.as_ref(), "route_scope")?;
        let adapter_code = required(adapter_code.as_ref(), "adapter_code")?;
        let account_fingerprint =
            credential_safe_digest(account_fingerprint.as_ref(), "account_fingerprint")?;
        let config_revision = credential_safe_digest(config_revision.as_ref(), "config_revision")?;
        Ok(Self {
            provider_endpoint,
            endpoint_origin,
            route_scope,
            adapter_code,
            observation_origin,
            account_fingerprint,
            config_revision,
        })
    }

    pub fn provider_endpoint(&self) -> &ProviderEndpointKey {
        &self.provider_endpoint
    }

    pub fn endpoint_origin(&self) -> &str {
        &self.endpoint_origin
    }

    pub fn route_scope(&self) -> &str {
        &self.route_scope
    }

    pub fn adapter_code(&self) -> &str {
        &self.adapter_code
    }

    pub fn observation_origin(&self) -> &str {
        &self.observation_origin
    }

    pub fn account_fingerprint(&self) -> &str {
        &self.account_fingerprint
    }

    pub fn config_revision(&self) -> &str {
        &self.config_revision
    }

    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        for value in [
            self.provider_endpoint.service_name.as_str(),
            self.provider_endpoint.provider_id.as_str(),
            self.provider_endpoint.endpoint_id.as_str(),
            self.endpoint_origin.as_str(),
            self.route_scope.as_str(),
            self.adapter_code.as_str(),
            self.observation_origin.as_str(),
            self.account_fingerprint.as_str(),
            self.config_revision.as_str(),
        ] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        format!("sha256:{:x}", hasher.finalize())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderObservationAuthority {
    Authoritative,
    Informational,
}

impl ProviderObservationAuthority {
    fn as_str(self) -> &'static str {
        match self {
            Self::Authoritative => "authoritative",
            Self::Informational => "informational",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "authoritative" => Ok(Self::Authoritative),
            "informational" => Ok(Self::Informational),
            other => Err(invalid_metadata(
                path,
                format!("provider observation has invalid authority {other:?}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderPolicyEffect {
    ObserveOnly {
        reason: String,
    },
    Block {
        action_kind: String,
        code: Option<String>,
        reason: String,
        expires_at_unix_ms: Option<u64>,
    },
    Recover {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderObservation {
    pub observed_at_unix_ms: u64,
    pub completed_at_unix_ms: u64,
    pub authority: ProviderObservationAuthority,
    pub evidence: Value,
    pub effect: ProviderPolicyEffect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAutomaticEligibility {
    Eligible,
    Blocked,
}

impl ProviderAutomaticEligibility {
    fn as_str(self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::Blocked => "blocked",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "eligible" => Ok(Self::Eligible),
            "blocked" => Ok(Self::Blocked),
            other => Err(invalid_metadata(
                path,
                format!("provider has invalid automatic eligibility {other:?}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderManualEligibility {
    Enabled,
    Disabled,
    Draining,
}

impl ProviderManualEligibility {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Draining => "draining",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            "draining" => Ok(Self::Draining),
            other => Err(invalid_metadata(
                path,
                format!("provider has invalid manual eligibility {other:?}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEffectiveEligibility {
    Eligible,
    Ineligible,
}

impl ProviderEffectiveEligibility {
    fn from_parts(
        automatic: ProviderAutomaticEligibility,
        manual: ProviderManualEligibility,
    ) -> Self {
        if automatic == ProviderAutomaticEligibility::Eligible
            && manual == ProviderManualEligibility::Enabled
        {
            Self::Eligible
        } else {
            Self::Ineligible
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::Ineligible => "ineligible",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "eligible" => Ok(Self::Eligible),
            "ineligible" => Ok(Self::Ineligible),
            other => Err(invalid_metadata(
                path,
                format!("provider has invalid effective eligibility {other:?}"),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPolicyActionRecord {
    pub action_id: Uuid,
    pub provider_endpoint: ProviderEndpointKey,
    pub incarnation_id: Uuid,
    pub generation: u64,
    pub action_kind: String,
    pub code: Option<String>,
    pub reason: String,
    pub opened_at_unix_ms: u64,
    pub expires_at_unix_ms: Option<u64>,
    pub closed_at_unix_ms: Option<u64>,
    pub close_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEligibilityProjection {
    pub provider_endpoint: ProviderEndpointKey,
    pub incarnation_id: Option<Uuid>,
    pub automatic: ProviderAutomaticEligibility,
    pub manual: ProviderManualEligibility,
    pub effective: ProviderEffectiveEligibility,
    pub active_action: Option<ProviderPolicyActionRecord>,
    pub manual_reason: Option<String>,
    pub updated_at_unix_ms: u64,
    pub policy_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPolicySnapshot {
    pub policy_revision: u64,
    pub projections: Vec<ProviderEligibilityProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservationTicket {
    store_id: Uuid,
    provider_endpoint: ProviderEndpointKey,
    incarnation_id: Uuid,
    generation: u64,
}

impl ProviderObservationTicket {
    pub fn provider_endpoint(&self) -> &ProviderEndpointKey {
        &self.provider_endpoint
    }

    pub fn incarnation_id(&self) -> Uuid {
        self.incarnation_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservationReservation {
    pub ticket: ProviderObservationTicket,
    pub scope_digest: String,
    route_scope: String,
    pub policy_revision: u64,
    pub projection: ProviderEligibilityProjection,
}

impl ProviderObservationReservation {
    pub(crate) fn route_scope(&self) -> &str {
        self.route_scope.as_str()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderObservationDisposition {
    Accepted,
    IgnoredStale,
    IgnoredInactiveIncarnation,
}

impl ProviderObservationDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::IgnoredStale => "ignored_stale",
            Self::IgnoredInactiveIncarnation => "ignored_inactive_incarnation",
        }
    }

    fn parse(path: &Path, value: &str) -> Result<Self, RuntimeStoreError> {
        match value {
            "accepted" => Ok(Self::Accepted),
            "ignored_stale" => Ok(Self::IgnoredStale),
            "ignored_inactive_incarnation" => Ok(Self::IgnoredInactiveIncarnation),
            other => Err(invalid_metadata(
                path,
                format!("provider observation has invalid disposition {other:?}"),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObservationCommit {
    pub disposition: ProviderObservationDisposition,
    pub policy_revision: u64,
    pub projection: ProviderEligibilityProjection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderObservationHistoryEntry {
    pub provider_endpoint: ProviderEndpointKey,
    pub incarnation_id: Uuid,
    pub scope_digest: String,
    pub generation: u64,
    pub observation: ProviderObservation,
    pub disposition: ProviderObservationDisposition,
    pub policy_revision: u64,
}

pub(super) fn reserve_provider_observation(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    scope: ProviderObservationScope,
    reserved_at_unix_ms: u64,
) -> Result<ProviderObservationReservation, RuntimeStoreError> {
    let reserved_at = checked_integer(
        reserved_at_unix_ms,
        "provider observation",
        scope.provider_endpoint.stable_key(),
        "reserved_at_unix_ms",
    )?;
    let store_id_text = store_id.to_string();
    let endpoint_json = encode_endpoint(&scope.provider_endpoint)?;
    validate_current_runtime_scope(
        transaction,
        path,
        store_id,
        &scope.provider_endpoint,
        &endpoint_json,
        &scope.route_scope,
    )?;
    let route_scope = scope.route_scope.clone();
    let scope_digest = scope.digest();
    ensure_revision_row(transaction, path, store_id, reserved_at)?;
    let current = transaction
        .query_row(
            "SELECT h.incarnation_id, i.scope_digest, i.last_reserved_generation
             FROM provider_endpoint_heads h
             JOIN provider_incarnations i
               ON i.store_id = h.store_id AND i.incarnation_id = h.incarnation_id
             WHERE h.store_id = ?1 AND h.endpoint_key_json = ?2",
            params![store_id_text, endpoint_json],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read provider observation head", source))?;

    let (incarnation_id, generation, policy_revision) = match current {
        Some((incarnation_bytes, current_digest, generation)) if current_digest == scope_digest => {
            let incarnation_id = decode_uuid(path, incarnation_bytes, "incarnation_id")?;
            let generation = decode_positive(path, generation, "last_reserved_generation")?
                .checked_add(1)
                .ok_or_else(|| policy_invariant(&scope.provider_endpoint, "generation overflow"))?;
            transaction
                .execute(
                    "UPDATE provider_incarnations SET last_reserved_generation = ?3
                     WHERE store_id = ?1 AND incarnation_id = ?2",
                    params![
                        store_id.to_string(),
                        incarnation_id.as_bytes().as_slice(),
                        checked_integer(
                            generation,
                            "provider observation",
                            scope.provider_endpoint.stable_key(),
                            "generation",
                        )?
                    ],
                )
                .map_err(|source| {
                    sqlite_error(path, "reserve provider observation generation", source)
                })?;
            (
                incarnation_id,
                generation,
                current_policy_revision(transaction, path, store_id)?,
            )
        }
        current => {
            if let Some((incarnation_bytes, _, _)) = current {
                let old_incarnation = decode_uuid(path, incarnation_bytes, "incarnation_id")?;
                transaction
                    .execute(
                        "UPDATE provider_incarnations SET deactivated_at_unix_ms = ?3
                         WHERE store_id = ?1 AND incarnation_id = ?2",
                        params![
                            store_id.to_string(),
                            old_incarnation.as_bytes().as_slice(),
                            reserved_at
                        ],
                    )
                    .map_err(|source| {
                        sqlite_error(path, "deactivate provider incarnation", source)
                    })?;
                close_active_action(
                    transaction,
                    path,
                    store_id,
                    &endpoint_json,
                    reserved_at,
                    "incarnation_changed",
                )?;
            }
            let incarnation_id = Uuid::new_v4();
            transaction
                .execute(
                    "INSERT INTO provider_incarnations (
                        store_id, incarnation_id, endpoint_key_json, scope_digest,
                        endpoint_origin, route_scope, adapter_code, observation_origin,
                        account_fingerprint, config_revision, activated_at_unix_ms,
                        last_reserved_generation, last_accepted_generation
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, 0)",
                    params![
                        store_id.to_string(),
                        incarnation_id.as_bytes().as_slice(),
                        endpoint_json,
                        scope_digest,
                        scope.endpoint_origin,
                        scope.route_scope,
                        scope.adapter_code,
                        scope.observation_origin,
                        scope.account_fingerprint,
                        scope.config_revision,
                        reserved_at,
                    ],
                )
                .map_err(|source| sqlite_error(path, "create provider incarnation", source))?;
            transaction
                .execute(
                    "INSERT INTO provider_endpoint_heads (store_id, endpoint_key_json, incarnation_id)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(store_id, endpoint_key_json) DO UPDATE SET
                        incarnation_id = excluded.incarnation_id",
                    params![
                        store_id.to_string(),
                        endpoint_json,
                        incarnation_id.as_bytes().as_slice()
                    ],
                )
                .map_err(|source| sqlite_error(path, "publish provider incarnation", source))?;
            let policy_revision =
                increment_policy_revision(transaction, path, store_id, reserved_at)?;
            let (manual, manual_reason) = read_manual(transaction, path, store_id, &endpoint_json)?;
            upsert_projection(
                transaction,
                path,
                store_id,
                &endpoint_json,
                Some(incarnation_id),
                ProviderAutomaticEligibility::Eligible,
                manual,
                None,
                manual_reason.as_deref(),
                reserved_at,
                policy_revision,
            )?;
            (incarnation_id, 1, policy_revision)
        }
    };
    let projection = read_projection(transaction, path, store_id, &scope.provider_endpoint)?
        .ok_or_else(|| {
            policy_invariant(
                &scope.provider_endpoint,
                "eligibility projection is missing",
            )
        })?;
    Ok(ProviderObservationReservation {
        ticket: ProviderObservationTicket {
            store_id,
            provider_endpoint: scope.provider_endpoint,
            incarnation_id,
            generation,
        },
        scope_digest,
        route_scope,
        policy_revision,
        projection,
    })
}

pub(super) fn commit_provider_observation(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    ticket: ProviderObservationTicket,
    observation: ProviderObservation,
) -> Result<ProviderObservationCommit, RuntimeStoreError> {
    if ticket.store_id != store_id {
        return Err(RuntimeStoreError::ForeignStoreHandle {
            entity: "provider observation ticket",
            id: format!("{}:{}", ticket.incarnation_id, ticket.generation),
            expected: store_id,
            actual: ticket.store_id,
        });
    }
    validate_observation(&ticket.provider_endpoint, &observation)?;
    let observed_at = checked_integer(
        observation.observed_at_unix_ms,
        "provider observation",
        ticket.provider_endpoint.stable_key(),
        "observed_at_unix_ms",
    )?;
    let completed_at = checked_integer(
        observation.completed_at_unix_ms,
        "provider observation",
        ticket.provider_endpoint.stable_key(),
        "completed_at_unix_ms",
    )?;
    let endpoint_json = encode_endpoint(&ticket.provider_endpoint)?;
    let incarnation = transaction
        .query_row(
            "SELECT scope_digest, last_accepted_generation
             FROM provider_incarnations
             WHERE store_id = ?1 AND incarnation_id = ?2 AND endpoint_key_json = ?3",
            params![
                store_id.to_string(),
                ticket.incarnation_id.as_bytes().as_slice(),
                endpoint_json
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read provider incarnation", source))?
        .ok_or_else(|| {
            policy_invariant(&ticket.provider_endpoint, "ticket incarnation is unknown")
        })?;
    let active_incarnation = transaction
        .query_row(
            "SELECT incarnation_id FROM provider_endpoint_heads
             WHERE store_id = ?1 AND endpoint_key_json = ?2",
            params![store_id.to_string(), endpoint_json],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read active provider incarnation", source))?
        .map(|value| decode_uuid(path, value, "active incarnation_id"))
        .transpose()?;
    let last_accepted = decode_nonnegative(path, incarnation.1, "last_accepted_generation")?;
    let disposition = if active_incarnation != Some(ticket.incarnation_id) {
        ProviderObservationDisposition::IgnoredInactiveIncarnation
    } else if ticket.generation <= last_accepted {
        ProviderObservationDisposition::IgnoredStale
    } else {
        ProviderObservationDisposition::Accepted
    };
    let mut projection = read_projection(transaction, path, store_id, &ticket.provider_endpoint)?
        .ok_or_else(|| {
        policy_invariant(
            &ticket.provider_endpoint,
            "eligibility projection is missing",
        )
    })?;
    let mut policy_revision = current_policy_revision(transaction, path, store_id)?;

    if disposition == ProviderObservationDisposition::Accepted {
        transaction
            .execute(
                "UPDATE provider_incarnations SET last_accepted_generation = ?3
                 WHERE store_id = ?1 AND incarnation_id = ?2",
                params![
                    store_id.to_string(),
                    ticket.incarnation_id.as_bytes().as_slice(),
                    checked_integer(
                        ticket.generation,
                        "provider observation",
                        ticket.provider_endpoint.stable_key(),
                        "generation",
                    )?
                ],
            )
            .map_err(|source| sqlite_error(path, "accept provider observation", source))?;

        let mut automatic = projection.automatic;
        let mut active_action_id = projection
            .active_action
            .as_ref()
            .map(|action| action.action_id);
        if observation.authority == ProviderObservationAuthority::Authoritative {
            match &observation.effect {
                ProviderPolicyEffect::ObserveOnly { .. } => {}
                ProviderPolicyEffect::Block {
                    action_kind,
                    code,
                    reason,
                    expires_at_unix_ms,
                } => {
                    close_active_action(
                        transaction,
                        path,
                        store_id,
                        &endpoint_json,
                        completed_at,
                        "superseded",
                    )?;
                    let action_id = Uuid::new_v4();
                    let expires_at = expires_at_unix_ms
                        .map(|value| {
                            checked_integer(
                                value,
                                "provider policy action",
                                action_id.to_string(),
                                "expires_at_unix_ms",
                            )
                        })
                        .transpose()?;
                    transaction
                        .execute(
                            "INSERT INTO provider_policy_actions (
                                store_id, action_id, endpoint_key_json, incarnation_id,
                                generation, action_kind, code, reason, opened_at_unix_ms,
                                expires_at_unix_ms
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                            params![
                                store_id.to_string(),
                                action_id.as_bytes().as_slice(),
                                endpoint_json,
                                ticket.incarnation_id.as_bytes().as_slice(),
                                checked_integer(
                                    ticket.generation,
                                    "provider policy action",
                                    action_id.to_string(),
                                    "generation",
                                )?,
                                action_kind,
                                code,
                                reason,
                                completed_at,
                                expires_at,
                            ],
                        )
                        .map_err(|source| {
                            sqlite_error(path, "open provider policy action", source)
                        })?;
                    automatic = ProviderAutomaticEligibility::Blocked;
                    active_action_id = Some(action_id);
                }
                ProviderPolicyEffect::Recover { .. } => {
                    close_active_action(
                        transaction,
                        path,
                        store_id,
                        &endpoint_json,
                        completed_at,
                        "authoritative_recovery",
                    )?;
                    automatic = ProviderAutomaticEligibility::Eligible;
                    active_action_id = None;
                }
            }
        }
        policy_revision = increment_policy_revision(transaction, path, store_id, completed_at)?;
        upsert_projection(
            transaction,
            path,
            store_id,
            &endpoint_json,
            Some(ticket.incarnation_id),
            automatic,
            projection.manual,
            active_action_id,
            projection.manual_reason.as_deref(),
            completed_at,
            policy_revision,
        )?;
        projection = read_projection(transaction, path, store_id, &ticket.provider_endpoint)?
            .ok_or_else(|| {
                policy_invariant(&ticket.provider_endpoint, "updated projection is missing")
            })?;
    }

    let evidence_json = serde_json::to_string(&observation.evidence).map_err(|source| {
        policy_invariant(
            &ticket.provider_endpoint,
            format!("evidence cannot be serialized: {source}"),
        )
    })?;
    let effect_json = serde_json::to_string(&observation.effect).map_err(|source| {
        policy_invariant(
            &ticket.provider_endpoint,
            format!("effect cannot be serialized: {source}"),
        )
    })?;
    transaction
        .execute(
            "INSERT INTO provider_observations (
                store_id, endpoint_key_json, incarnation_id, generation,
                observed_at_unix_ms, completed_at_unix_ms, authority,
                evidence_json, effect_json, disposition, policy_revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                store_id.to_string(),
                endpoint_json,
                ticket.incarnation_id.as_bytes().as_slice(),
                checked_integer(
                    ticket.generation,
                    "provider observation",
                    ticket.provider_endpoint.stable_key(),
                    "generation",
                )?,
                observed_at,
                completed_at,
                observation.authority.as_str(),
                evidence_json,
                effect_json,
                disposition.as_str(),
                checked_integer(
                    policy_revision,
                    "provider policy",
                    store_id.to_string(),
                    "policy_revision",
                )?,
            ],
        )
        .map_err(|source| sqlite_error(path, "record provider observation", source))?;

    Ok(ProviderObservationCommit {
        disposition,
        policy_revision,
        projection,
    })
}

pub(super) fn set_provider_manual_eligibility(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    provider_endpoint: ProviderEndpointKey,
    manual: ProviderManualEligibility,
    reason: Option<String>,
    updated_at_unix_ms: u64,
) -> Result<ProviderEligibilityProjection, RuntimeStoreError> {
    validate_endpoint_key(&provider_endpoint)
        .map_err(|source| policy_invariant(&provider_endpoint, source.to_string()))?;
    let reason = reason
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if manual != ProviderManualEligibility::Enabled && reason.is_none() {
        return Err(policy_invariant(
            &provider_endpoint,
            "disabled or draining manual eligibility requires a reason",
        ));
    }
    let updated_at = checked_integer(
        updated_at_unix_ms,
        "provider manual eligibility",
        provider_endpoint.stable_key(),
        "updated_at_unix_ms",
    )?;
    let endpoint_json = encode_endpoint(&provider_endpoint)?;
    ensure_revision_row(transaction, path, store_id, updated_at)?;
    transaction
        .execute(
            "INSERT INTO provider_manual_eligibility (
                store_id, endpoint_key_json, eligibility, reason, updated_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(store_id, endpoint_key_json) DO UPDATE SET
                eligibility = excluded.eligibility,
                reason = excluded.reason,
                updated_at_unix_ms = excluded.updated_at_unix_ms",
            params![
                store_id.to_string(),
                endpoint_json,
                manual.as_str(),
                reason,
                updated_at
            ],
        )
        .map_err(|source| sqlite_error(path, "set provider manual eligibility", source))?;
    let existing = read_projection(transaction, path, store_id, &provider_endpoint)?;
    let policy_revision = increment_policy_revision(transaction, path, store_id, updated_at)?;
    upsert_projection(
        transaction,
        path,
        store_id,
        &endpoint_json,
        existing.as_ref().and_then(|value| value.incarnation_id),
        existing
            .as_ref()
            .map_or(ProviderAutomaticEligibility::Eligible, |value| {
                value.automatic
            }),
        manual,
        existing
            .as_ref()
            .and_then(|value| value.active_action.as_ref())
            .map(|action| action.action_id),
        reason.as_deref(),
        updated_at,
        policy_revision,
    )?;
    read_projection(transaction, path, store_id, &provider_endpoint)?
        .ok_or_else(|| policy_invariant(&provider_endpoint, "updated manual projection is missing"))
}

pub(super) fn reconcile_runtime_upstream_identities(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    identities: &[RuntimeUpstreamIdentity],
    updated_at_unix_ms: u64,
) -> Result<ProviderPolicySnapshot, RuntimeStoreError> {
    let mut expected_scopes = BTreeMap::new();
    for identity in identities {
        let scope = identity.policy_route_scope();
        if let Some(previous) =
            expected_scopes.insert(identity.provider_endpoint.clone(), scope.clone())
            && previous != scope
        {
            return Err(policy_invariant(
                &identity.provider_endpoint,
                "runtime snapshot contains conflicting upstream identities",
            ));
        }
    }

    let updated_at = checked_integer(
        updated_at_unix_ms,
        "runtime identity reconciliation",
        store_id.to_string(),
        "updated_at_unix_ms",
    )?;
    transaction
        .execute(
            "INSERT INTO runtime_identity_authority (store_id, updated_at_unix_ms)
             VALUES (?1, ?2)
             ON CONFLICT(store_id) DO UPDATE SET updated_at_unix_ms = excluded.updated_at_unix_ms",
            params![store_id.to_string(), updated_at],
        )
        .map_err(|source| sqlite_error(path, "publish runtime identity authority", source))?;
    transaction
        .execute(
            "DELETE FROM runtime_upstream_identities WHERE store_id = ?1",
            [store_id.to_string()],
        )
        .map_err(|source| sqlite_error(path, "replace runtime upstream identities", source))?;
    for (endpoint, route_scope) in &expected_scopes {
        transaction
            .execute(
                "INSERT INTO runtime_upstream_identities (
                    store_id, endpoint_key_json, route_scope
                 ) VALUES (?1, ?2, ?3)",
                params![
                    store_id.to_string(),
                    encode_endpoint(endpoint)?,
                    route_scope
                ],
            )
            .map_err(|source| sqlite_error(path, "publish runtime upstream identity", source))?;
    }

    let mut statement = transaction
        .prepare(
            "SELECT h.endpoint_key_json, h.incarnation_id, i.route_scope
             FROM provider_endpoint_heads h
             JOIN provider_incarnations i
               ON i.store_id = h.store_id AND i.incarnation_id = h.incarnation_id
             WHERE h.store_id = ?1
             ORDER BY h.endpoint_key_json ASC",
        )
        .map_err(|source| sqlite_error(path, "prepare runtime identity reconciliation", source))?;
    let rows = statement
        .query_map([store_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|source| sqlite_error(path, "read runtime identity reconciliation", source))?;
    let mut invalidated = Vec::new();
    for row in rows {
        let (endpoint_json, incarnation_bytes, route_scope) = row.map_err(|source| {
            sqlite_error(path, "decode runtime identity reconciliation", source)
        })?;
        let endpoint = decode_endpoint(path, &endpoint_json)?;
        if expected_scopes
            .get(&endpoint)
            .is_some_and(|expected| expected == &route_scope)
        {
            continue;
        }
        invalidated.push((
            endpoint,
            endpoint_json,
            decode_uuid(path, incarnation_bytes, "runtime identity incarnation_id")?,
        ));
    }
    drop(statement);

    if invalidated.is_empty() {
        return provider_policy_snapshot(transaction, path, store_id);
    }

    ensure_revision_row(transaction, path, store_id, updated_at)?;
    let policy_revision = increment_policy_revision(transaction, path, store_id, updated_at)?;

    for (endpoint, endpoint_json, incarnation_id) in invalidated {
        transaction
            .execute(
                "UPDATE provider_incarnations
                 SET deactivated_at_unix_ms = MAX(activated_at_unix_ms, ?3)
                 WHERE store_id = ?1 AND incarnation_id = ?2",
                params![
                    store_id.to_string(),
                    incarnation_id.as_bytes().as_slice(),
                    updated_at
                ],
            )
            .map_err(|source| sqlite_error(path, "deactivate replaced runtime identity", source))?;
        let deleted = transaction
            .execute(
                "DELETE FROM provider_endpoint_heads
                 WHERE store_id = ?1 AND endpoint_key_json = ?2 AND incarnation_id = ?3",
                params![
                    store_id.to_string(),
                    endpoint_json,
                    incarnation_id.as_bytes().as_slice()
                ],
            )
            .map_err(|source| {
                sqlite_error(path, "remove replaced runtime identity head", source)
            })?;
        if deleted != 1 {
            return Err(policy_invariant(
                &endpoint,
                "active runtime identity disappeared during reconciliation",
            ));
        }
        close_active_action(
            transaction,
            path,
            store_id,
            &endpoint_json,
            updated_at,
            "runtime_identity_changed",
        )?;
        let (manual, manual_reason) = read_manual(transaction, path, store_id, &endpoint_json)?;
        upsert_projection(
            transaction,
            path,
            store_id,
            &endpoint_json,
            None,
            ProviderAutomaticEligibility::Eligible,
            manual,
            None,
            manual_reason.as_deref(),
            updated_at,
            policy_revision,
        )?;
    }

    provider_policy_snapshot(transaction, path, store_id)
}

fn validate_current_runtime_scope(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    provider_endpoint: &ProviderEndpointKey,
    endpoint_json: &str,
    route_scope: &str,
) -> Result<(), RuntimeStoreError> {
    if current_runtime_scope_is_active(connection, path, store_id, endpoint_json, route_scope)? {
        return Ok(());
    }
    Err(policy_invariant(
        provider_endpoint,
        "provider observation scope is not active in the current runtime snapshot",
    ))
}

fn current_runtime_scope_is_active(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    endpoint_json: &str,
    route_scope: &str,
) -> Result<bool, RuntimeStoreError> {
    let authority_exists = connection
        .query_row(
            "SELECT 1 FROM runtime_identity_authority WHERE store_id = ?1",
            [store_id.to_string()],
            |_| Ok(()),
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read runtime identity authority", source))?
        .is_some();
    if !authority_exists {
        return Ok(true);
    }
    let current_scope = connection
        .query_row(
            "SELECT route_scope FROM runtime_upstream_identities
             WHERE store_id = ?1 AND endpoint_key_json = ?2",
            params![store_id.to_string(), endpoint_json],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read current runtime upstream identity", source))?;
    Ok(current_scope.as_deref() == Some(route_scope))
}

pub(super) fn runtime_upstream_identity_is_active(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    identity: &RuntimeUpstreamIdentity,
) -> Result<bool, RuntimeStoreError> {
    let endpoint_json = encode_endpoint(&identity.provider_endpoint)?;
    current_runtime_scope_is_active(
        connection,
        path,
        store_id,
        &endpoint_json,
        &identity.policy_route_scope(),
    )
}

pub(super) fn provider_policy_snapshot(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
) -> Result<ProviderPolicySnapshot, RuntimeStoreError> {
    let policy_revision = current_policy_revision(connection, path, store_id)?;
    let mut statement = connection
        .prepare(
            "SELECT endpoint_key_json FROM provider_eligibility
             WHERE store_id = ?1 ORDER BY endpoint_key_json ASC",
        )
        .map_err(|source| sqlite_error(path, "prepare provider policy snapshot", source))?;
    let endpoint_rows = statement
        .query_map([store_id.to_string()], |row| row.get::<_, String>(0))
        .map_err(|source| sqlite_error(path, "read provider policy snapshot", source))?;
    let mut projections = Vec::new();
    for endpoint_json in endpoint_rows {
        let endpoint_json = endpoint_json
            .map_err(|source| sqlite_error(path, "decode provider policy endpoint", source))?;
        let endpoint = decode_endpoint(path, &endpoint_json)?;
        projections.push(
            read_projection(connection, path, store_id, &endpoint)?.ok_or_else(|| {
                policy_invariant(&endpoint, "snapshot projection disappeared while reading")
            })?,
        );
    }
    Ok(ProviderPolicySnapshot {
        policy_revision,
        projections,
    })
}

pub(super) fn read_provider_observation_history(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    provider_endpoint: &ProviderEndpointKey,
    limit: u64,
) -> Result<Vec<ProviderObservationHistoryEntry>, RuntimeStoreError> {
    let endpoint_json = encode_endpoint(provider_endpoint)?;
    let limit = checked_integer(
        limit,
        "provider observation history",
        provider_endpoint.stable_key(),
        "limit",
    )?;
    let mut statement = connection
        .prepare(
            "SELECT o.incarnation_id, i.scope_digest, o.generation,
                    o.observed_at_unix_ms, o.completed_at_unix_ms, o.authority,
                    o.evidence_json, o.effect_json, o.disposition, o.policy_revision
             FROM provider_observations o
             JOIN provider_incarnations i
               ON i.store_id = o.store_id AND i.incarnation_id = o.incarnation_id
             WHERE o.store_id = ?1 AND o.endpoint_key_json = ?2
             ORDER BY i.activated_at_unix_ms ASC, o.generation ASC
             LIMIT ?3",
        )
        .map_err(|source| sqlite_error(path, "prepare provider observation history", source))?;
    let rows = statement
        .query_map(params![store_id.to_string(), endpoint_json, limit], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })
        .map_err(|source| sqlite_error(path, "read provider observation history", source))?;
    let mut history = Vec::new();
    for row in rows {
        let (
            incarnation_id,
            scope_digest,
            generation,
            observed_at,
            completed_at,
            authority,
            evidence_json,
            effect_json,
            disposition,
            policy_revision,
        ) = row.map_err(|source| {
            sqlite_error(path, "decode provider observation history row", source)
        })?;
        history.push(ProviderObservationHistoryEntry {
            provider_endpoint: provider_endpoint.clone(),
            incarnation_id: decode_uuid(path, incarnation_id, "incarnation_id")?,
            scope_digest,
            generation: decode_positive(path, generation, "generation")?,
            observation: ProviderObservation {
                observed_at_unix_ms: decode_nonnegative(path, observed_at, "observed_at")?,
                completed_at_unix_ms: decode_nonnegative(path, completed_at, "completed_at")?,
                authority: ProviderObservationAuthority::parse(path, &authority)?,
                evidence: serde_json::from_str(&evidence_json).map_err(|source| {
                    invalid_metadata(path, format!("invalid observation evidence: {source}"))
                })?,
                effect: serde_json::from_str(&effect_json).map_err(|source| {
                    invalid_metadata(path, format!("invalid observation effect: {source}"))
                })?,
            },
            disposition: ProviderObservationDisposition::parse(path, &disposition)?,
            policy_revision: decode_nonnegative(path, policy_revision, "policy_revision")?,
        });
    }
    Ok(history)
}

fn read_projection(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    provider_endpoint: &ProviderEndpointKey,
) -> Result<Option<ProviderEligibilityProjection>, RuntimeStoreError> {
    let endpoint_json = encode_endpoint(provider_endpoint)?;
    let row = connection
        .query_row(
            "SELECT e.incarnation_id, e.automatic_eligibility, e.manual_eligibility,
                    e.effective_eligibility, e.manual_reason, e.updated_at_unix_ms,
                    e.policy_revision,
                    a.action_id, a.incarnation_id, a.generation, a.action_kind,
                    a.code, a.reason, a.opened_at_unix_ms, a.expires_at_unix_ms,
                    a.closed_at_unix_ms, a.close_reason
             FROM provider_eligibility e
             LEFT JOIN provider_policy_actions a
               ON a.store_id = e.store_id AND a.action_id = e.active_action_id
             WHERE e.store_id = ?1 AND e.endpoint_key_json = ?2",
            params![store_id.to_string(), endpoint_json],
            |row| {
                Ok((
                    row.get::<_, Option<Vec<u8>>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<Vec<u8>>>(7)?,
                    row.get::<_, Option<Vec<u8>>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                ))
            },
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read provider eligibility projection", source))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let automatic = ProviderAutomaticEligibility::parse(path, &row.1)?;
    let manual = ProviderManualEligibility::parse(path, &row.2)?;
    let effective = ProviderEffectiveEligibility::parse(path, &row.3)?;
    if effective != ProviderEffectiveEligibility::from_parts(automatic, manual) {
        return Err(invalid_metadata(
            path,
            "provider effective eligibility conflicts with precedence",
        ));
    }
    let active_action = match row.7 {
        None => None,
        Some(action_id) => Some(ProviderPolicyActionRecord {
            action_id: decode_uuid(path, action_id, "action_id")?,
            provider_endpoint: provider_endpoint.clone(),
            incarnation_id: decode_uuid(
                path,
                row.8
                    .ok_or_else(|| invalid_metadata(path, "action incarnation is missing"))?,
                "action incarnation_id",
            )?,
            generation: decode_positive(
                path,
                row.9
                    .ok_or_else(|| invalid_metadata(path, "action generation is missing"))?,
                "action generation",
            )?,
            action_kind: row
                .10
                .ok_or_else(|| invalid_metadata(path, "action kind is missing"))?,
            code: row.11,
            reason: row
                .12
                .ok_or_else(|| invalid_metadata(path, "action reason is missing"))?,
            opened_at_unix_ms: decode_nonnegative(
                path,
                row.13
                    .ok_or_else(|| invalid_metadata(path, "action opened_at is missing"))?,
                "action opened_at",
            )?,
            expires_at_unix_ms: row
                .14
                .map(|value| decode_nonnegative(path, value, "action expires_at"))
                .transpose()?,
            closed_at_unix_ms: row
                .15
                .map(|value| decode_nonnegative(path, value, "action closed_at"))
                .transpose()?,
            close_reason: row.16,
        }),
    };
    Ok(Some(ProviderEligibilityProjection {
        provider_endpoint: provider_endpoint.clone(),
        incarnation_id: row
            .0
            .map(|value| decode_uuid(path, value, "projection incarnation_id"))
            .transpose()?,
        automatic,
        manual,
        effective,
        active_action,
        manual_reason: row.4,
        updated_at_unix_ms: decode_nonnegative(path, row.5, "projection updated_at")?,
        policy_revision: decode_nonnegative(path, row.6, "projection policy_revision")?,
    }))
}

#[allow(clippy::too_many_arguments)]
fn upsert_projection(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    endpoint_json: &str,
    incarnation_id: Option<Uuid>,
    automatic: ProviderAutomaticEligibility,
    manual: ProviderManualEligibility,
    active_action_id: Option<Uuid>,
    manual_reason: Option<&str>,
    updated_at: i64,
    policy_revision: u64,
) -> Result<(), RuntimeStoreError> {
    let effective = ProviderEffectiveEligibility::from_parts(automatic, manual);
    transaction
        .execute(
            "INSERT INTO provider_eligibility (
                store_id, endpoint_key_json, incarnation_id, automatic_eligibility,
                manual_eligibility, effective_eligibility, active_action_id,
                manual_reason, updated_at_unix_ms, policy_revision
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(store_id, endpoint_key_json) DO UPDATE SET
                incarnation_id = excluded.incarnation_id,
                automatic_eligibility = excluded.automatic_eligibility,
                manual_eligibility = excluded.manual_eligibility,
                effective_eligibility = excluded.effective_eligibility,
                active_action_id = excluded.active_action_id,
                manual_reason = excluded.manual_reason,
                updated_at_unix_ms = excluded.updated_at_unix_ms,
                policy_revision = excluded.policy_revision",
            params![
                store_id.to_string(),
                endpoint_json,
                incarnation_id.map(|value| value.as_bytes().to_vec()),
                automatic.as_str(),
                manual.as_str(),
                effective.as_str(),
                active_action_id.map(|value| value.as_bytes().to_vec()),
                manual_reason,
                updated_at,
                checked_integer(
                    policy_revision,
                    "provider policy",
                    store_id.to_string(),
                    "policy_revision",
                )?,
            ],
        )
        .map_err(|source| sqlite_error(path, "publish provider eligibility", source))?;
    Ok(())
}

fn read_manual(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
    endpoint_json: &str,
) -> Result<(ProviderManualEligibility, Option<String>), RuntimeStoreError> {
    let row = connection
        .query_row(
            "SELECT eligibility, reason FROM provider_manual_eligibility
             WHERE store_id = ?1 AND endpoint_key_json = ?2",
            params![store_id.to_string(), endpoint_json],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read provider manual eligibility", source))?;
    row.map(|(manual, reason)| {
        ProviderManualEligibility::parse(path, &manual).map(|manual| (manual, reason))
    })
    .transpose()
    .map(|value| value.unwrap_or((ProviderManualEligibility::Enabled, None)))
}

fn close_active_action(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    endpoint_json: &str,
    closed_at: i64,
    reason: &str,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE provider_policy_actions
             SET closed_at_unix_ms = MAX(opened_at_unix_ms, ?3), close_reason = ?4
             WHERE store_id = ?1 AND endpoint_key_json = ?2
               AND closed_at_unix_ms IS NULL",
            params![store_id.to_string(), endpoint_json, closed_at, reason],
        )
        .map_err(|source| sqlite_error(path, "close provider policy action", source))?;
    Ok(())
}

fn ensure_revision_row(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    updated_at: i64,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "INSERT INTO runtime_revisions (store_id, policy_revision, updated_at_unix_ms)
             VALUES (?1, 0, ?2) ON CONFLICT(store_id) DO NOTHING",
            params![store_id.to_string(), updated_at],
        )
        .map_err(|source| sqlite_error(path, "initialize runtime policy revision", source))?;
    Ok(())
}

fn increment_policy_revision(
    transaction: &Transaction<'_>,
    path: &Path,
    store_id: Uuid,
    updated_at: i64,
) -> Result<u64, RuntimeStoreError> {
    let revision = transaction
        .query_row(
            "UPDATE runtime_revisions
             SET policy_revision = policy_revision + 1, updated_at_unix_ms = ?2
             WHERE store_id = ?1 RETURNING policy_revision",
            params![store_id.to_string(), updated_at],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| sqlite_error(path, "advance runtime policy revision", source))?;
    decode_nonnegative(path, revision, "policy_revision")
}

fn current_policy_revision(
    connection: &Connection,
    path: &Path,
    store_id: Uuid,
) -> Result<u64, RuntimeStoreError> {
    let revision = connection
        .query_row(
            "SELECT policy_revision FROM runtime_revisions WHERE store_id = ?1",
            [store_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|source| sqlite_error(path, "read runtime policy revision", source))?
        .unwrap_or(0);
    decode_nonnegative(path, revision, "policy_revision")
}

fn validate_observation(
    endpoint: &ProviderEndpointKey,
    observation: &ProviderObservation,
) -> Result<(), RuntimeStoreError> {
    if observation.completed_at_unix_ms < observation.observed_at_unix_ms {
        return Err(policy_invariant(
            endpoint,
            "completed_at_unix_ms precedes observed_at_unix_ms",
        ));
    }
    let required = match &observation.effect {
        ProviderPolicyEffect::ObserveOnly { reason } | ProviderPolicyEffect::Recover { reason } => {
            vec![("reason", reason.as_str())]
        }
        ProviderPolicyEffect::Block {
            action_kind,
            reason,
            ..
        } => vec![
            ("action_kind", action_kind.as_str()),
            ("reason", reason.as_str()),
        ],
    };
    if let Some((field, _)) = required.iter().find(|(_, value)| value.trim().is_empty()) {
        return Err(policy_invariant(endpoint, format!("{field} is empty")));
    }
    Ok(())
}

fn canonical_origin(
    value: &str,
    field: &'static str,
) -> Result<String, ProviderObservationScopeError> {
    let url = Url::parse(value.trim())
        .map_err(|_| ProviderObservationScopeError::InvalidOrigin(field))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ProviderObservationScopeError::InvalidOrigin(field));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProviderObservationScopeError::CredentialsNotAllowed(field));
    }
    let origin = url.origin().ascii_serialization();
    if origin == "null" {
        Err(ProviderObservationScopeError::InvalidOrigin(field))
    } else {
        Ok(origin)
    }
}

fn required(value: &str, field: &'static str) -> Result<String, ProviderObservationScopeError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ProviderObservationScopeError::EmptyField(field))
    } else {
        Ok(value.to_string())
    }
}

fn credential_safe_digest(
    value: &str,
    field: &'static str,
) -> Result<String, ProviderObservationScopeError> {
    let value = value.trim();
    if value.len() <= "sha256:".len() || !value.starts_with("sha256:") {
        Err(ProviderObservationScopeError::CredentialSafeDigestRequired(
            field,
        ))
    } else {
        Ok(value.to_string())
    }
}

fn validate_endpoint_key(
    endpoint: &ProviderEndpointKey,
) -> Result<(), ProviderObservationScopeError> {
    for (field, value) in [
        ("provider service_name", endpoint.service_name.as_str()),
        ("provider_id", endpoint.provider_id.as_str()),
        ("endpoint_id", endpoint.endpoint_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ProviderObservationScopeError::EmptyField(field));
        }
    }
    Ok(())
}

fn encode_endpoint(endpoint: &ProviderEndpointKey) -> Result<String, RuntimeStoreError> {
    serde_json::to_string(endpoint).map_err(|source| {
        policy_invariant(
            endpoint,
            format!("endpoint key cannot be serialized: {source}"),
        )
    })
}

fn decode_endpoint(path: &Path, encoded: &str) -> Result<ProviderEndpointKey, RuntimeStoreError> {
    let endpoint = serde_json::from_str::<ProviderEndpointKey>(encoded)
        .map_err(|source| invalid_metadata(path, format!("invalid provider endpoint: {source}")))?;
    validate_endpoint_key(&endpoint)
        .map_err(|source| invalid_metadata(path, source.to_string()))?;
    Ok(endpoint)
}

fn checked_integer(
    value: u64,
    entity: &'static str,
    id: String,
    field: &'static str,
) -> Result<i64, RuntimeStoreError> {
    i64::try_from(value).map_err(|_| RuntimeStoreError::InvariantViolation {
        entity,
        id,
        detail: format!("{field} exceeds SQLite integer range"),
    })
}

fn decode_uuid(path: &Path, value: Vec<u8>, field: &str) -> Result<Uuid, RuntimeStoreError> {
    Uuid::from_slice(&value)
        .map_err(|_| invalid_metadata(path, format!("{field} is not UUID BLOB16")))
}

fn decode_nonnegative(path: &Path, value: i64, field: &str) -> Result<u64, RuntimeStoreError> {
    u64::try_from(value).map_err(|_| invalid_metadata(path, format!("{field} is negative")))
}

fn decode_positive(path: &Path, value: i64, field: &str) -> Result<u64, RuntimeStoreError> {
    let value = decode_nonnegative(path, value, field)?;
    if value == 0 {
        Err(invalid_metadata(path, format!("{field} must be positive")))
    } else {
        Ok(value)
    }
}

fn policy_invariant(
    endpoint: &ProviderEndpointKey,
    detail: impl Into<String>,
) -> RuntimeStoreError {
    RuntimeStoreError::InvariantViolation {
        entity: "provider policy",
        id: endpoint.stable_key(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

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

    fn scope(account: &str, endpoint_origin: &str) -> ProviderObservationScope {
        scope_for("primary", account, endpoint_origin)
    }

    fn scope_for(
        provider_id: &str,
        account: &str,
        endpoint_origin: &str,
    ) -> ProviderObservationScope {
        scope_for_config(provider_id, account, endpoint_origin, "sha256:runtime-one")
    }

    fn scope_for_config(
        provider_id: &str,
        account: &str,
        endpoint_origin: &str,
        config_revision: &str,
    ) -> ProviderObservationScope {
        ProviderObservationScope::new(
            ProviderEndpointKey::new("codex", provider_id, "default"),
            endpoint_origin,
            "route:codex/main",
            "sub2api_usage",
            "https://console.example.test/v1/usage",
            account,
            config_revision,
        )
        .expect("valid observation scope")
    }

    fn scope_for_runtime_identity(identity: &RuntimeUpstreamIdentity) -> ProviderObservationScope {
        ProviderObservationScope::new(
            identity.provider_endpoint.clone(),
            identity.base_url.as_str(),
            identity.policy_route_scope(),
            "sub2api_usage",
            "https://console.example.test/v1/usage",
            "sha256:account-a",
            "sha256:runtime-one",
        )
        .expect("valid runtime identity observation scope")
    }

    fn observation(effect: ProviderPolicyEffect) -> ProviderObservation {
        ProviderObservation {
            observed_at_unix_ms: 100,
            completed_at_unix_ms: 200,
            authority: ProviderObservationAuthority::Authoritative,
            evidence: json!({"remaining": 0}),
            effect,
        }
    }

    fn block(reason: &str) -> ProviderPolicyEffect {
        ProviderPolicyEffect::Block {
            action_kind: "cooldown".to_string(),
            code: Some("quota_exhausted".to_string()),
            reason: reason.to_string(),
            expires_at_unix_ms: Some(60_000),
        }
    }

    #[test]
    fn scope_digest_is_canonical_credential_free_and_keeps_origins_independent() {
        let scope = ProviderObservationScope::new(
            ProviderEndpointKey::new("codex", "primary", "default"),
            "HTTPS://API.EXAMPLE.TEST:443/v1?ignored=true",
            " route:codex/main ",
            " sub2api_usage ",
            "https://console.example.test/v1/usage?ignored=true",
            "sha256:account-a",
            "sha256:runtime-one",
        )
        .expect("valid scope");
        assert_eq!(scope.endpoint_origin(), "https://api.example.test");
        assert_eq!(scope.observation_origin(), "https://console.example.test");
        assert_ne!(scope.endpoint_origin(), scope.observation_origin());
        assert!(scope.digest().starts_with("sha256:"));
        assert!(!scope.digest().contains("account-a"));
    }

    #[test]
    fn slow_older_generation_cannot_override_newer_recovery() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let first = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                10,
            )
            .expect("reserve generation one");
        let second = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                11,
            )
            .expect("reserve generation two");
        assert_eq!(first.ticket.generation(), 1);
        assert_eq!(second.ticket.generation(), 2);

        let recovered = store
            .commit_provider_observation(
                second.ticket,
                observation(ProviderPolicyEffect::Recover {
                    reason: "daily_reset".to_string(),
                }),
            )
            .expect("commit newer recovery");
        assert_eq!(
            recovered.disposition,
            ProviderObservationDisposition::Accepted
        );
        assert_eq!(
            recovered.projection.effective,
            ProviderEffectiveEligibility::Eligible
        );

        let stale = store
            .commit_provider_observation(first.ticket, observation(block("late exhausted")))
            .expect("record stale completion");
        assert_eq!(
            stale.disposition,
            ProviderObservationDisposition::IgnoredStale
        );
        assert_eq!(stale.policy_revision, recovered.policy_revision);
        assert_eq!(stale.projection, recovered.projection);

        let history = store
            .read_provider_observation_history(&recovered.projection.provider_endpoint, 10)
            .expect("read observation history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].generation, 1);
        assert_eq!(
            history[0].disposition,
            ProviderObservationDisposition::IgnoredStale
        );
        assert_eq!(history[1].generation, 2);
        assert_eq!(
            history[1].disposition,
            ProviderObservationDisposition::Accepted
        );
    }

    #[test]
    fn older_recovery_cannot_override_newer_exhaustion() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let _generation_one = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                10,
            )
            .expect("reserve generation one");
        let recovery = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                11,
            )
            .expect("reserve generation two recovery");
        let exhausted = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                12,
            )
            .expect("reserve generation three exhaustion");

        let blocked = store
            .commit_provider_observation(exhausted.ticket, observation(block("newer exhaustion")))
            .expect("commit newer exhaustion");
        assert_eq!(
            blocked.disposition,
            ProviderObservationDisposition::Accepted
        );
        assert_eq!(
            blocked.projection.automatic,
            ProviderAutomaticEligibility::Blocked
        );

        let stale = store
            .commit_provider_observation(
                recovery.ticket,
                observation(ProviderPolicyEffect::Recover {
                    reason: "late reset".to_string(),
                }),
            )
            .expect("record stale recovery");
        assert_eq!(
            stale.disposition,
            ProviderObservationDisposition::IgnoredStale
        );
        assert_eq!(stale.policy_revision, blocked.policy_revision);
        assert_eq!(stale.projection, blocked.projection);
    }

    #[test]
    fn automatic_policy_is_isolated_between_sibling_endpoints() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let primary = store
            .reserve_provider_observation(
                scope_for("primary", "sha256:account-a", "https://api.example.test/v1"),
                10,
            )
            .expect("reserve primary");
        let sibling = store
            .reserve_provider_observation(
                scope_for("backup", "sha256:account-a", "https://api.example.test/v1"),
                11,
            )
            .expect("reserve sibling");
        assert_eq!(primary.ticket.generation(), 1);
        assert_eq!(sibling.ticket.generation(), 1);

        store
            .commit_provider_observation(primary.ticket, observation(block("primary exhausted")))
            .expect("block primary");

        let snapshot = store
            .provider_policy_snapshot()
            .expect("read policy snapshot");
        let primary = snapshot
            .projections
            .iter()
            .find(|projection| projection.provider_endpoint.provider_id == "primary")
            .expect("primary projection");
        let sibling = snapshot
            .projections
            .iter()
            .find(|projection| projection.provider_endpoint.provider_id == "backup")
            .expect("sibling projection");
        assert_eq!(primary.automatic, ProviderAutomaticEligibility::Blocked);
        assert_eq!(primary.effective, ProviderEffectiveEligibility::Ineligible);
        assert_eq!(sibling.automatic, ProviderAutomaticEligibility::Eligible);
        assert_eq!(sibling.effective, ProviderEffectiveEligibility::Eligible);
        assert!(sibling.active_action.is_none());
    }

    #[test]
    fn informational_and_observe_only_observations_do_not_mutate_eligibility() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let initial = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                10,
            )
            .and_then(|reservation| {
                store.commit_provider_observation(
                    reservation.ticket,
                    observation(block("quota exhausted")),
                )
            })
            .expect("commit initial block");
        let initial_action = initial
            .projection
            .active_action
            .as_ref()
            .expect("active block action")
            .action_id;

        let informational = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                11,
            )
            .and_then(|reservation| {
                let mut candidate = observation(ProviderPolicyEffect::Recover {
                    reason: "untrusted reset".to_string(),
                });
                candidate.authority = ProviderObservationAuthority::Informational;
                store.commit_provider_observation(reservation.ticket, candidate)
            })
            .expect("commit informational recovery");
        assert_eq!(
            informational.projection.automatic,
            ProviderAutomaticEligibility::Blocked
        );
        assert_eq!(
            informational
                .projection
                .active_action
                .as_ref()
                .expect("informational observation preserves action")
                .action_id,
            initial_action
        );

        let observe_only = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                12,
            )
            .and_then(|reservation| {
                store.commit_provider_observation(
                    reservation.ticket,
                    observation(ProviderPolicyEffect::ObserveOnly {
                        reason: "parse failed".to_string(),
                    }),
                )
            })
            .expect("commit authoritative observe-only observation");
        assert_eq!(
            observe_only.projection.automatic,
            ProviderAutomaticEligibility::Blocked
        );
        assert_eq!(
            observe_only
                .projection
                .active_action
                .as_ref()
                .expect("observe-only preserves action")
                .action_id,
            initial_action
        );
    }

    #[test]
    fn old_incarnation_cannot_mutate_policy_after_identity_or_config_change() {
        let cases = [
            (
                "account",
                "sha256:account-b",
                "https://api.example.test/v1",
                "sha256:runtime-one",
            ),
            (
                "endpoint origin",
                "sha256:account-a",
                "https://api-new.example.test/v1",
                "sha256:runtime-one",
            ),
            (
                "config revision",
                "sha256:account-a",
                "https://api.example.test/v1",
                "sha256:runtime-two",
            ),
        ];

        for (changed_dimension, account, endpoint_origin, config_revision) in cases {
            let store = RuntimeStore::open_in_memory().expect("open store");
            let initial_scope = scope_for_config(
                "primary",
                "sha256:account-a",
                "https://api.example.test/v1",
                "sha256:runtime-one",
            );
            let initial = store
                .reserve_provider_observation(initial_scope.clone(), 10)
                .and_then(|reservation| {
                    store.commit_provider_observation(
                        reservation.ticket,
                        observation(block("initial exhaustion")),
                    )
                })
                .expect("commit initial block");
            assert_eq!(
                initial.projection.automatic,
                ProviderAutomaticEligibility::Blocked
            );
            let slow_old = store
                .reserve_provider_observation(initial_scope, 11)
                .expect("reserve slow old observation");
            let current = store
                .reserve_provider_observation(
                    scope_for_config("primary", account, endpoint_origin, config_revision),
                    12,
                )
                .expect("reserve current incarnation");
            assert_ne!(
                slow_old.ticket.incarnation_id(),
                current.ticket.incarnation_id(),
                "{changed_dimension} must create a new incarnation"
            );
            assert_ne!(slow_old.scope_digest, current.scope_digest);
            assert_eq!(
                current.projection.automatic,
                ProviderAutomaticEligibility::Eligible
            );
            assert!(current.projection.active_action.is_none());

            let ignored = store
                .commit_provider_observation(
                    slow_old.ticket,
                    observation(block("late old incarnation")),
                )
                .expect("record inactive completion");
            assert_eq!(
                ignored.disposition,
                ProviderObservationDisposition::IgnoredInactiveIncarnation
            );
            assert_eq!(ignored.policy_revision, current.policy_revision);
            assert_eq!(
                ignored.projection.effective,
                ProviderEffectiveEligibility::Eligible
            );
        }
    }

    #[test]
    fn runtime_identity_reconciliation_resets_automatic_policy_and_preserves_manual_intent() {
        let endpoint = ProviderEndpointKey::new("codex", "primary", "default");
        let original = RuntimeUpstreamIdentity::new_with_continuity_domain(
            endpoint.clone(),
            "https://old.example.test/v1",
            Some("continuity-a".to_string()),
        );
        let replacements = [
            RuntimeUpstreamIdentity::new_with_continuity_domain(
                endpoint.clone(),
                "https://new.example.test/v1",
                Some("continuity-a".to_string()),
            ),
            RuntimeUpstreamIdentity::new_with_continuity_domain(
                endpoint.clone(),
                "https://old.example.test/v1",
                Some("continuity-b".to_string()),
            ),
        ];

        for replacement in replacements {
            let store = RuntimeStore::open_in_memory().expect("open store");
            store
                .reserve_provider_observation(scope_for_runtime_identity(&original), 10)
                .and_then(|reservation| {
                    store.commit_provider_observation(
                        reservation.ticket,
                        observation(block("initial exhaustion")),
                    )
                })
                .expect("commit initial runtime identity block");
            store
                .set_provider_manual_eligibility(
                    endpoint.clone(),
                    ProviderManualEligibility::Disabled,
                    Some("operator stop".to_string()),
                    11,
                )
                .expect("commit manual disable");
            let slow_old = store
                .reserve_provider_observation(scope_for_runtime_identity(&original), 12)
                .expect("reserve slow old runtime identity observation");
            let slow_old_incarnation = slow_old.ticket.incarnation_id();

            let retained = store
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&original), 13)
                .expect("retain matching runtime identity");
            assert_eq!(retained.policy_revision, slow_old.policy_revision);
            assert_eq!(
                retained.projections[0].automatic,
                ProviderAutomaticEligibility::Blocked
            );

            let reconciled = store
                .reconcile_runtime_upstream_identities(std::slice::from_ref(&replacement), 14)
                .expect("reconcile replaced runtime identity");
            let projection = reconciled.projections.first().expect("policy projection");
            assert_eq!(projection.automatic, ProviderAutomaticEligibility::Eligible);
            assert_eq!(projection.manual, ProviderManualEligibility::Disabled);
            assert_eq!(
                projection.effective,
                ProviderEffectiveEligibility::Ineligible
            );
            assert!(projection.active_action.is_none());
            assert!(projection.incarnation_id.is_none());
            assert!(reconciled.policy_revision > retained.policy_revision);

            let ignored = store
                .commit_provider_observation(
                    slow_old.ticket,
                    observation(block("late old runtime identity")),
                )
                .expect("record inactive old runtime identity observation");
            assert_eq!(
                ignored.disposition,
                ProviderObservationDisposition::IgnoredInactiveIncarnation
            );
            assert_eq!(ignored.projection, projection.clone());

            let current = store
                .reserve_provider_observation(scope_for_runtime_identity(&replacement), 15)
                .expect("reserve replacement runtime identity observation");
            assert_ne!(current.ticket.incarnation_id(), slow_old_incarnation);
            assert_eq!(
                current.projection.automatic,
                ProviderAutomaticEligibility::Eligible
            );
            assert_eq!(
                current.projection.manual,
                ProviderManualEligibility::Disabled
            );
        }
    }

    #[test]
    fn runtime_identity_reconciliation_rejects_reservation_from_replaced_scope() {
        let endpoint = ProviderEndpointKey::new("codex", "primary", "default");
        let original = RuntimeUpstreamIdentity::new_with_continuity_domain(
            endpoint.clone(),
            "https://old.example.test/v1",
            Some("continuity-a".to_string()),
        );
        let replacement = RuntimeUpstreamIdentity::new_with_continuity_domain(
            endpoint,
            "https://new.example.test/v1",
            Some("continuity-a".to_string()),
        );
        let store = RuntimeStore::open_in_memory().expect("open store");

        store
            .reconcile_runtime_upstream_identities(std::slice::from_ref(&original), 10)
            .expect("publish original runtime identity");
        store
            .reconcile_runtime_upstream_identities(std::slice::from_ref(&replacement), 11)
            .expect("replace runtime identity");

        let error = store
            .reserve_provider_observation(scope_for_runtime_identity(&original), 12)
            .expect_err("replaced runtime scope must not reactivate itself");
        assert!(matches!(
            error,
            RuntimeStoreError::InvariantViolation { .. }
        ));
        assert!(error.to_string().contains("current runtime snapshot"));

        store
            .reserve_provider_observation(scope_for_runtime_identity(&replacement), 13)
            .expect("current runtime scope remains reservable");
    }

    #[test]
    fn manual_disable_survives_automatic_block_and_recovery() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let endpoint = ProviderEndpointKey::new("codex", "primary", "default");
        store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                10,
            )
            .expect("reserve initial incarnation");
        store
            .set_provider_manual_eligibility(
                endpoint.clone(),
                ProviderManualEligibility::Disabled,
                Some("operator stop".to_string()),
                11,
            )
            .expect("disable endpoint manually");

        let exhausted = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                12,
            )
            .and_then(|reservation| {
                store.commit_provider_observation(
                    reservation.ticket,
                    observation(block("quota exhausted")),
                )
            })
            .expect("commit automatic block");
        assert_eq!(
            exhausted.projection.effective,
            ProviderEffectiveEligibility::Ineligible
        );

        let recovered = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                13,
            )
            .and_then(|reservation| {
                store.commit_provider_observation(
                    reservation.ticket,
                    observation(ProviderPolicyEffect::Recover {
                        reason: "quota reset".to_string(),
                    }),
                )
            })
            .expect("commit automatic recovery");
        assert_eq!(
            recovered.projection.automatic,
            ProviderAutomaticEligibility::Eligible
        );
        assert_eq!(
            recovered.projection.manual,
            ProviderManualEligibility::Disabled
        );
        assert_eq!(
            recovered.projection.effective,
            ProviderEffectiveEligibility::Ineligible
        );
    }

    #[test]
    fn injected_policy_failure_rolls_back_observation_action_projection_and_revision() {
        let store = RuntimeStore::open_in_memory().expect("open store");
        let initial = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                10,
            )
            .and_then(|reservation| {
                store.commit_provider_observation(
                    reservation.ticket,
                    observation(block("quota exhausted")),
                )
            })
            .expect("commit initial block");
        let before = store.provider_policy_snapshot().expect("read LKG snapshot");
        let history_len = store
            .read_provider_observation_history(&initial.projection.provider_endpoint, 10)
            .expect("read initial history")
            .len();

        let recovery = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                20,
            )
            .expect("reserve recovery");
        store.fail_next_policy_commit_for_test();
        let error = store
            .commit_provider_observation(
                recovery.ticket,
                observation(ProviderPolicyEffect::Recover {
                    reason: "quota reset".to_string(),
                }),
            )
            .expect_err("injected failure must roll back");
        assert!(matches!(error, RuntimeStoreError::InjectedFailure { .. }));
        assert_eq!(store.provider_policy_snapshot().expect("read LKG"), before);
        assert_eq!(
            store
                .read_provider_observation_history(&initial.projection.provider_endpoint, 10)
                .expect("read rolled-back history")
                .len(),
            history_len
        );
    }

    #[test]
    fn provider_policy_round_trips_across_reopen() {
        let home = TestDir::new("policy-reopen");
        let store = RuntimeStore::open_in_home(home.path()).expect("open store");
        let committed = store
            .reserve_provider_observation(
                scope("sha256:account-a", "https://api.example.test/v1"),
                10,
            )
            .and_then(|reservation| {
                store.commit_provider_observation(
                    reservation.ticket,
                    observation(block("quota exhausted")),
                )
            })
            .expect("commit policy");
        let expected = store.provider_policy_snapshot().expect("read snapshot");
        drop(store);

        let reopened = RuntimeStore::open_in_home(home.path()).expect("reopen store");
        assert_eq!(
            reopened.provider_policy_snapshot().expect("restore policy"),
            expected
        );
        let reader = RuntimeStoreReader::open_in_home(home.path()).expect("open reader");
        assert_eq!(
            reader.provider_policy_snapshot().expect("read policy"),
            expected
        );
        assert_eq!(expected.policy_revision, committed.policy_revision);
    }
}
