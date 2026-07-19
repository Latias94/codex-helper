use std::fmt;
use std::path::Path;

use thiserror::Error;
use uuid::Uuid;

use crate::runtime_store::{RuntimeStore, RuntimeStoreError, RuntimeStoreReader};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallationIdentityErrorCode {
    Busy,
    Invalid,
    PermissionDenied,
    Unavailable,
}

#[derive(Clone, Copy, PartialEq, Eq, Error)]
#[error("installation identity is {code}")]
pub struct InstallationIdentityError {
    code: InstallationIdentityErrorCode,
}

impl InstallationIdentityError {
    pub fn code(&self) -> InstallationIdentityErrorCode {
        self.code
    }
}

impl fmt::Debug for InstallationIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstallationIdentityError")
            .field("code", &self.code)
            .finish()
    }
}

impl fmt::Display for InstallationIdentityErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "busy",
            Self::Invalid => "invalid",
            Self::PermissionDenied => "permission_denied",
            Self::Unavailable => "unavailable",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstallationIdentity(Uuid);

impl InstallationIdentity {
    pub fn resolve_default() -> Result<Self, InstallationIdentityError> {
        Self::resolve_in_home(crate::config::proxy_home_dir())
    }

    pub fn resolve_in_home(
        helper_home: impl AsRef<Path>,
    ) -> Result<Self, InstallationIdentityError> {
        let helper_home = helper_home.as_ref();
        match RuntimeStoreReader::open_in_home(helper_home) {
            Ok(reader) => Ok(Self(reader.identity().store_id())),
            Err(RuntimeStoreError::DatabaseMissing { .. }) => {
                let store = RuntimeStore::open_in_home(helper_home).map_err(map_store_error)?;
                Ok(Self(store.identity().store_id()))
            }
            Err(error) => Err(map_store_error(error)),
        }
    }

    pub fn uuid(self) -> Uuid {
        self.0
    }

    pub(crate) fn from_runtime_store(runtime_store: &RuntimeStore) -> Self {
        Self(runtime_store.identity().store_id())
    }

    #[cfg(test)]
    pub(super) fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

fn map_store_error(error: RuntimeStoreError) -> InstallationIdentityError {
    let code = match error {
        RuntimeStoreError::WriterAlreadyOwned { .. } => InstallationIdentityErrorCode::Busy,
        RuntimeStoreError::CorruptDatabase { .. }
        | RuntimeStoreError::IntegrityCheckFailed { .. }
        | RuntimeStoreError::InvalidMetadata { .. }
        | RuntimeStoreError::ForeignApplication { .. }
        | RuntimeStoreError::UnsupportedSchemaRevision { .. }
        | RuntimeStoreError::UnidentifiedNonemptyDatabase { .. }
        | RuntimeStoreError::UnsafeDatabasePath { .. }
        | RuntimeStoreError::UnsafeWriterLease { .. } => InstallationIdentityErrorCode::Invalid,
        RuntimeStoreError::CreateDirectory { ref source, .. }
        | RuntimeStoreError::OpenWriterLease { ref source, .. }
        | RuntimeStoreError::AcquireWriterLease { ref source, .. }
        | RuntimeStoreError::SecurePermissions { ref source, .. }
        | RuntimeStoreError::InspectDatabasePath { ref source, .. }
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            InstallationIdentityErrorCode::PermissionDenied
        }
        _ => InstallationIdentityErrorCode::Unavailable,
    };
    InstallationIdentityError { code }
}
