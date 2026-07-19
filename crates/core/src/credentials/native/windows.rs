use std::collections::HashMap;

use keyring_core::api::CredentialStoreApi as _;
use keyring_core::{Entry, Error as KeyringError};
use windows_native_keyring_store::Store;
use zeroize::Zeroize as _;

use super::{LocatorLocks, NativeCredentialLocator};
use crate::credentials::capabilities::{
    NATIVE_CREDENTIAL_MAX_BYTES, NativeCredentialStore, NativeStoreError, NativeStoreErrorCode,
};
use crate::credentials::model::{CredentialValueError, SecretValue};

const WINDOWS_SERVICE: &str = "codex-helper";
const WINDOWS_USER: &str = "native-credential";

#[derive(Default)]
pub(super) struct WindowsNativeCredentialStore {
    locks: LocatorLocks,
}

impl WindowsNativeCredentialStore {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn entry(locator: &NativeCredentialLocator) -> Result<Entry, NativeStoreError> {
        let modifiers = HashMap::from([("target", locator.as_str()), ("persistence", "Local")]);
        Store::new()
            .map_err(map_keyring_error)?
            .build(WINDOWS_SERVICE, WINDOWS_USER, Some(&modifiers))
            .map_err(map_keyring_error)
    }

    fn exists(entry: &Entry) -> Result<bool, NativeStoreError> {
        match entry.get_attributes() {
            Ok(_) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(source) => Err(map_keyring_error(source)),
        }
    }
}

impl NativeCredentialStore for WindowsNativeCredentialStore {
    fn create(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        validate_windows_secret_size(value.expose())?;
        self.locks.with_lock(locator, || {
            let entry = Self::entry(locator)?;
            if Self::exists(&entry)? {
                return Err(error(NativeStoreErrorCode::AlreadyExists));
            }
            entry.set_secret(value.expose()).map_err(map_keyring_error)
        })
    }

    fn set(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        validate_windows_secret_size(value.expose())?;
        self.locks.with_lock(locator, || {
            let entry = Self::entry(locator)?;
            entry.set_secret(value.expose()).map_err(map_keyring_error)
        })
    }

    fn read(&self, locator: &NativeCredentialLocator) -> Result<SecretValue, NativeStoreError> {
        self.locks.with_lock(locator, || {
            let bytes = Self::entry(locator)?
                .get_secret()
                .map_err(map_keyring_error)?;
            SecretValue::new(bytes).map_err(map_value_error)
        })
    }

    fn delete(&self, locator: &NativeCredentialLocator) -> Result<(), NativeStoreError> {
        self.locks.with_lock(locator, || {
            Self::entry(locator)?
                .delete_credential()
                .map_err(map_keyring_error)
        })
    }
}

fn validate_windows_secret_size(bytes: &[u8]) -> Result<(), NativeStoreError> {
    if bytes.len() > NATIVE_CREDENTIAL_MAX_BYTES {
        return Err(error(NativeStoreErrorCode::Invalid));
    }
    Ok(())
}

fn map_keyring_error(error_value: KeyringError) -> NativeStoreError {
    match error_value {
        KeyringError::NoEntry => error(NativeStoreErrorCode::Missing),
        KeyringError::BadEncoding(mut bytes) => {
            bytes.zeroize();
            error(NativeStoreErrorCode::Invalid)
        }
        KeyringError::BadDataFormat(mut bytes, _) => {
            bytes.zeroize();
            error(NativeStoreErrorCode::Invalid)
        }
        KeyringError::BadStoreFormat(_)
        | KeyringError::TooLong(_, _)
        | KeyringError::Invalid(_, _) => error(NativeStoreErrorCode::Invalid),
        KeyringError::Ambiguous(_) => error(NativeStoreErrorCode::Ambiguous),
        KeyringError::NoStorageAccess(_) => error(NativeStoreErrorCode::BackendUnavailable),
        KeyringError::NoDefaultStore => error(NativeStoreErrorCode::BackendUnavailable),
        KeyringError::NotSupportedByStore(_) => error(NativeStoreErrorCode::Unsupported),
        KeyringError::PlatformFailure(_) => error(NativeStoreErrorCode::BackendUnavailable),
        _ => error(NativeStoreErrorCode::BackendUnavailable),
    }
}

fn map_value_error(_error: CredentialValueError) -> NativeStoreError {
    error(NativeStoreErrorCode::Invalid)
}

fn error(code: NativeStoreErrorCode) -> NativeStoreError {
    NativeStoreError::new(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_secret_size_accepts_2560_bytes_and_rejects_2561() {
        assert!(validate_windows_secret_size(&vec![b'a'; 2_559]).is_ok());
        assert!(validate_windows_secret_size(&vec![b'a'; 2_560]).is_ok());
        assert_eq!(
            validate_windows_secret_size(&vec![b'a'; 2_561])
                .expect_err("2,561-byte secret must fail")
                .code(),
            NativeStoreErrorCode::Invalid
        );
    }

    #[test]
    fn missing_windows_logon_session_is_backend_unavailable() {
        let source = KeyringError::NoStorageAccess(Box::new(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        assert_eq!(
            map_keyring_error(source).code(),
            NativeStoreErrorCode::BackendUnavailable
        );
    }
}
