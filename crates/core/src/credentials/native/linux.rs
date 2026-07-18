use std::collections::HashMap;

use secret_service::blocking::{Item, SecretService};
use secret_service::{EncryptionType, Error as SecretServiceError, SearchItemsResult};

use super::{LocatorLocks, NativeCredentialLocator};
use crate::credentials::capabilities::{
    NativeCredentialStore, NativeStoreError, NativeStoreErrorCode,
};
use crate::credentials::model::{CredentialValueError, SecretValue};

const ITEM_LABEL: &str = "codex-helper native credential";
const APPLICATION_ATTRIBUTE: &str = "codex-helper";
const CONTENT_TYPE: &str = "text/plain; charset=utf-8";

#[derive(Default)]
pub(super) struct LinuxNativeCredentialStore {
    locks: LocatorLocks,
}

impl LinuxNativeCredentialStore {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn connect() -> Result<SecretService<'static>, NativeStoreError> {
        SecretService::connect(native_encryption()).map_err(map_secret_service_error)
    }

    fn attributes(locator: &NativeCredentialLocator) -> HashMap<&str, &str> {
        HashMap::from([
            ("application", APPLICATION_ATTRIBUTE),
            ("credential-locator", locator.as_str()),
        ])
    }
}

fn native_encryption() -> EncryptionType {
    EncryptionType::Dh
}

impl NativeCredentialStore for LinuxNativeCredentialStore {
    fn create(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        self.locks.with_lock(locator, || {
            let service = Self::connect()?;
            let found = service
                .search_items(Self::attributes(locator))
                .map_err(map_secret_service_error)?;
            if search_selection(&found) != SearchSelection::Missing {
                return Err(error(if found.unlocked.len() + found.locked.len() > 1 {
                    NativeStoreErrorCode::Ambiguous
                } else {
                    NativeStoreErrorCode::AlreadyExists
                }));
            }
            let collection = service
                .get_default_collection()
                .map_err(map_secret_service_error)?;
            if collection.is_locked().map_err(map_secret_service_error)? {
                collection.unlock().map_err(map_secret_service_error)?;
            }
            collection
                .create_item(
                    ITEM_LABEL,
                    Self::attributes(locator),
                    value.expose(),
                    false,
                    CONTENT_TYPE,
                )
                .map_err(map_secret_service_error)?;
            Ok(())
        })
    }

    fn set(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        self.locks.with_lock(locator, || {
            let service = Self::connect()?;
            let found = service
                .search_items(Self::attributes(locator))
                .map_err(map_secret_service_error)?;
            let item = management_item(&found)?;
            if item.is_locked().map_err(map_secret_service_error)? {
                item.unlock().map_err(map_secret_service_error)?;
            }
            item.set_secret(value.expose(), CONTENT_TYPE)
                .map_err(map_secret_service_error)
        })
    }

    fn read(&self, locator: &NativeCredentialLocator) -> Result<SecretValue, NativeStoreError> {
        self.locks.with_lock(locator, || {
            let service = Self::connect()?;
            let found = service
                .search_items(Self::attributes(locator))
                .map_err(map_secret_service_error)?;
            match search_selection(&found) {
                SearchSelection::Missing => Err(error(NativeStoreErrorCode::Missing)),
                SearchSelection::Locked => Err(error(NativeStoreErrorCode::Locked)),
                SearchSelection::Ambiguous => Err(error(NativeStoreErrorCode::Ambiguous)),
                SearchSelection::Unlocked => {
                    let bytes = found.unlocked[0]
                        .get_secret()
                        .map_err(map_secret_service_error)?;
                    SecretValue::new(bytes).map_err(map_value_error)
                }
            }
        })
    }

    fn delete(&self, locator: &NativeCredentialLocator) -> Result<(), NativeStoreError> {
        self.locks.with_lock(locator, || {
            let service = Self::connect()?;
            let found = service
                .search_items(Self::attributes(locator))
                .map_err(map_secret_service_error)?;
            let item = management_item(&found)?;
            if item.is_locked().map_err(map_secret_service_error)? {
                item.unlock().map_err(map_secret_service_error)?;
            }
            item.delete().map_err(map_secret_service_error)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchSelection {
    Missing,
    Unlocked,
    Locked,
    Ambiguous,
}

fn search_selection<T>(found: &SearchItemsResult<T>) -> SearchSelection {
    match (found.unlocked.len(), found.locked.len()) {
        (0, 0) => SearchSelection::Missing,
        (1, 0) => SearchSelection::Unlocked,
        (0, 1) => SearchSelection::Locked,
        _ => SearchSelection::Ambiguous,
    }
}

fn management_item<'item>(
    found: &'item SearchItemsResult<Item<'item>>,
) -> Result<&'item Item<'item>, NativeStoreError> {
    match search_selection(found) {
        SearchSelection::Missing => Err(error(NativeStoreErrorCode::Missing)),
        SearchSelection::Ambiguous => Err(error(NativeStoreErrorCode::Ambiguous)),
        SearchSelection::Unlocked => Ok(&found.unlocked[0]),
        SearchSelection::Locked => Ok(&found.locked[0]),
    }
}

fn map_secret_service_error(source: SecretServiceError) -> NativeStoreError {
    match source {
        SecretServiceError::Locked => error(NativeStoreErrorCode::Locked),
        SecretServiceError::NoResult => error(NativeStoreErrorCode::Missing),
        SecretServiceError::Prompt => error(NativeStoreErrorCode::InteractionRequired),
        SecretServiceError::Unavailable => error(NativeStoreErrorCode::BackendUnavailable),
        SecretServiceError::ZbusFdo(zbus::fdo::Error::AccessDenied(_)) => {
            error(NativeStoreErrorCode::PermissionDenied)
        }
        SecretServiceError::Crypto(_)
        | SecretServiceError::Zbus(_)
        | SecretServiceError::ZbusFdo(_)
        | SecretServiceError::Zvariant(_) => error(NativeStoreErrorCode::BackendUnavailable),
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
    fn daemon_search_never_selects_locked_or_duplicate_items() {
        assert_eq!(
            search_selection(&SearchItemsResult::<()> {
                unlocked: vec![],
                locked: vec![()],
            }),
            SearchSelection::Locked
        );
        assert_eq!(
            search_selection(&SearchItemsResult::<()> {
                unlocked: vec![()],
                locked: vec![()],
            }),
            SearchSelection::Ambiguous
        );
        assert_eq!(
            search_selection(&SearchItemsResult::<()> {
                unlocked: vec![(), ()],
                locked: vec![],
            }),
            SearchSelection::Ambiguous
        );
    }

    #[test]
    fn linux_native_store_uses_an_encrypted_session() {
        assert_eq!(native_encryption(), EncryptionType::Dh);
    }

    #[test]
    fn dbus_access_denied_is_permission_denied() {
        let source =
            SecretServiceError::ZbusFdo(zbus::fdo::Error::AccessDenied("test denial".to_string()));
        assert_eq!(
            map_secret_service_error(source).code(),
            NativeStoreErrorCode::PermissionDenied
        );
    }
}
