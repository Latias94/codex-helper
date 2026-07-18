use std::collections::HashMap;

use apple_native_keyring_store::keychain::Store;
use keyring_core::api::CredentialStoreApi as _;
use keyring_core::{Entry, Error as KeyringError};
use security_framework::base::Error as SecurityError;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit, SearchResult};
use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};
use zeroize::{Zeroize, Zeroizing};

use super::{LocatorLocks, NativeCredentialLocator};
use crate::credentials::capabilities::{
    NativeCredentialStore, NativeStoreError, NativeStoreErrorCode,
};
use crate::credentials::model::{CredentialValueError, SecretValue};

const KEYCHAIN_SERVICE: &str = "codex-helper native credential";
const DAEMON_METADATA_QUERY_POLICY: MacosDaemonQueryPolicy = MacosDaemonQueryPolicy {
    load_attributes: true,
    load_data: false,
    limit_all: true,
    skip_authentication_ui: true,
};
const DAEMON_DATA_QUERY_POLICY: MacosDaemonQueryPolicy = MacosDaemonQueryPolicy {
    load_attributes: false,
    load_data: true,
    limit_all: true,
    skip_authentication_ui: true,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MacosDaemonQueryPolicy {
    load_attributes: bool,
    load_data: bool,
    limit_all: bool,
    skip_authentication_ui: bool,
}

#[derive(Default)]
pub(super) struct MacosNativeCredentialStore {
    locks: LocatorLocks,
}

impl MacosNativeCredentialStore {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn store() -> Result<std::sync::Arc<Store>, NativeStoreError> {
        Store::new().map_err(map_keyring_error)
    }

    fn entry(locator: &NativeCredentialLocator) -> Result<Entry, NativeStoreError> {
        Self::store()?
            .build(KEYCHAIN_SERVICE, locator.as_str(), None)
            .map_err(map_keyring_error)
    }

    fn search(locator: &NativeCredentialLocator) -> Result<Vec<Entry>, NativeStoreError> {
        let spec = HashMap::from([("service", KEYCHAIN_SERVICE), ("user", locator.as_str())]);
        Self::store()?.search(&spec).map_err(map_keyring_error)
    }
}

impl NativeCredentialStore for MacosNativeCredentialStore {
    fn create(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        self.locks
            .with_lock(locator, || match Self::search(locator)?.len() {
                0 => Self::entry(locator)?
                    .set_secret(value.expose())
                    .map_err(map_keyring_error),
                1 => Err(error(NativeStoreErrorCode::AlreadyExists)),
                _ => Err(error(NativeStoreErrorCode::Ambiguous)),
            })
    }

    fn set(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        self.locks.with_lock(locator, || {
            let mut entries = Self::search(locator)?;
            match entries.len() {
                0 => Err(error(NativeStoreErrorCode::Missing)),
                1 => entries
                    .pop()
                    .expect("one keychain entry")
                    .set_secret(value.expose())
                    .map_err(map_keyring_error),
                _ => Err(error(NativeStoreErrorCode::Ambiguous)),
            }
        })
    }

    fn read(&self, locator: &NativeCredentialLocator) -> Result<SecretValue, NativeStoreError> {
        self.locks.with_lock(locator, || {
            let metadata = query_login_keychain(locator, DAEMON_METADATA_QUERY_POLICY)?;
            match metadata.len() {
                0 => Err(error(NativeStoreErrorCode::Missing)),
                1 => finish_daemon_data_query(query_login_keychain(
                    locator,
                    DAEMON_DATA_QUERY_POLICY,
                )),
                _ => Err(error(NativeStoreErrorCode::Ambiguous)),
            }
        })
    }

    fn delete(&self, locator: &NativeCredentialLocator) -> Result<(), NativeStoreError> {
        self.locks.with_lock(locator, || {
            let mut entries = Self::search(locator)?;
            match entries.len() {
                0 => Err(error(NativeStoreErrorCode::Missing)),
                1 => entries
                    .pop()
                    .expect("one keychain entry")
                    .delete_credential()
                    .map_err(map_keyring_error),
                _ => Err(error(NativeStoreErrorCode::Ambiguous)),
            }
        })
    }
}

fn query_login_keychain(
    locator: &NativeCredentialLocator,
    policy: MacosDaemonQueryPolicy,
) -> Result<Vec<SearchResult>, NativeStoreError> {
    let keychain =
        SecKeychain::default_for_domain(SecPreferencesDomain::User).map_err(map_security_error)?;
    let keychains = [keychain];
    let mut options = ItemSearchOptions::new();
    options
        .keychains(&keychains)
        .class(ItemClass::generic_password())
        .service(KEYCHAIN_SERVICE)
        .account(locator.as_str())
        .load_attributes(policy.load_attributes)
        .load_data(policy.load_data)
        .skip_authenticated_items(policy.skip_authentication_ui);
    if policy.limit_all {
        options.limit(Limit::All);
    }

    options.search().map_err(map_security_error)
}

fn finish_daemon_data_query(
    query: Result<Vec<SearchResult>, NativeStoreError>,
) -> Result<SecretValue, NativeStoreError> {
    let results = match query {
        Ok(results) => results,
        Err(source) if source.code() == NativeStoreErrorCode::Missing => {
            return Err(error(NativeStoreErrorCode::InteractionRequired));
        }
        Err(source) => return Err(source),
    };
    let mut values = results
        .into_iter()
        .map(|result| match result {
            SearchResult::Data(value) => Ok(Zeroizing::new(value)),
            _ => Err(error(NativeStoreErrorCode::Invalid)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    match values.len() {
        0 => Err(error(NativeStoreErrorCode::InteractionRequired)),
        1 => SecretValue::from_zeroizing(values.pop().expect("one Keychain value"))
            .map_err(map_value_error),
        _ => Err(error(NativeStoreErrorCode::Ambiguous)),
    }
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
        KeyringError::NoDefaultStore => error(NativeStoreErrorCode::BackendUnavailable),
        KeyringError::NotSupportedByStore(_) => error(NativeStoreErrorCode::Unsupported),
        KeyringError::NoStorageAccess(platform) => {
            platform.downcast_ref::<SecurityError>().map_or_else(
                || error(NativeStoreErrorCode::PermissionDenied),
                |source| map_security_code(source.code()),
            )
        }
        KeyringError::PlatformFailure(platform) => {
            platform.downcast_ref::<SecurityError>().map_or_else(
                || error(NativeStoreErrorCode::BackendUnavailable),
                |source| map_security_code(source.code()),
            )
        }
        _ => error(NativeStoreErrorCode::BackendUnavailable),
    }
}

fn map_security_error(source: SecurityError) -> NativeStoreError {
    map_security_code(source.code())
}

fn map_security_code(code: i32) -> NativeStoreError {
    match code {
        -25300 => error(NativeStoreErrorCode::Missing),
        -25315 | -25308 | -128 => error(NativeStoreErrorCode::InteractionRequired),
        -25293 => error(NativeStoreErrorCode::Locked),
        -61 | -25292 => error(NativeStoreErrorCode::PermissionDenied),
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
    fn daemon_query_policy_forbids_authentication_ui() {
        assert_eq!(
            DAEMON_METADATA_QUERY_POLICY,
            MacosDaemonQueryPolicy {
                load_attributes: true,
                load_data: false,
                limit_all: true,
                skip_authentication_ui: true,
            }
        );
        assert_eq!(
            DAEMON_DATA_QUERY_POLICY,
            MacosDaemonQueryPolicy {
                load_attributes: false,
                load_data: true,
                limit_all: true,
                skip_authentication_ui: true,
            }
        );
    }

    #[test]
    fn existing_metadata_with_hidden_data_requires_interaction() {
        for data_query in [Ok(vec![]), Err(error(NativeStoreErrorCode::Missing))] {
            let error = match finish_daemon_data_query(data_query) {
                Ok(_) => panic!("hidden Keychain data must not be readable"),
                Err(error) => error,
            };
            assert_eq!(error.code(), NativeStoreErrorCode::InteractionRequired);
        }
    }

    #[test]
    fn daemon_security_errors_have_stable_noninteractive_categories() {
        assert_eq!(
            map_security_code(-25308).code(),
            NativeStoreErrorCode::InteractionRequired
        );
        assert_eq!(
            map_security_code(-25315).code(),
            NativeStoreErrorCode::InteractionRequired
        );
        assert_eq!(
            map_security_code(-25293).code(),
            NativeStoreErrorCode::Locked
        );
        assert_eq!(
            map_security_code(-61).code(),
            NativeStoreErrorCode::PermissionDenied
        );
        assert_eq!(
            map_security_code(-25300).code(),
            NativeStoreErrorCode::Missing
        );
    }
}
