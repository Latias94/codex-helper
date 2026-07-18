use std::fmt;
use std::sync::Arc;

use super::installation_identity::InstallationIdentity;
use super::model::{
    CredentialError, CredentialErrorCode, CredentialName, CredentialSourceKind, SecretValue,
};
use super::native::{NativeCredentialLocator, NativeCredentialNamespace};

pub(super) type NativeStoreErrorCode = CredentialErrorCode;
pub(super) const NATIVE_CREDENTIAL_MAX_BYTES: usize = 2_560;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct NativeStoreError {
    code: NativeStoreErrorCode,
}

impl NativeStoreError {
    #[cfg(any(feature = "native-credentials", test))]
    pub(super) fn new(code: NativeStoreErrorCode) -> Self {
        Self { code }
    }

    pub(super) fn code(self) -> NativeStoreErrorCode {
        self.code
    }
}

impl fmt::Debug for NativeStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeStoreError")
            .field("code", &self.code)
            .finish()
    }
}

impl fmt::Display for NativeStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "native credential backend failed: {:?}",
            self.code
        )
    }
}

impl std::error::Error for NativeStoreError {}

pub(super) trait NativeCredentialStore: Send + Sync {
    fn create(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError>;

    fn set(
        &self,
        locator: &NativeCredentialLocator,
        value: &SecretValue,
    ) -> Result<(), NativeStoreError>;

    fn read(&self, locator: &NativeCredentialLocator) -> Result<SecretValue, NativeStoreError>;

    fn delete(&self, locator: &NativeCredentialLocator) -> Result<(), NativeStoreError>;
}

#[derive(Clone, Default)]
pub struct CredentialSourceCapabilities {
    native: Option<Arc<dyn NativeCredentialStore>>,
}

impl CredentialSourceCapabilities {
    pub fn server() -> Self {
        Self { native: None }
    }

    pub fn native_supported(&self) -> bool {
        self.native.is_some()
    }

    #[cfg(feature = "native-credentials")]
    pub fn platform_native() -> Self {
        Self::with_backend(super::native::platform_store())
    }

    pub fn manager(&self, installation: InstallationIdentity) -> NativeCredentialManager {
        NativeCredentialManager {
            backend: self.native.clone(),
            namespace: NativeCredentialNamespace::new(installation),
        }
    }

    pub fn daemon(&self, installation: InstallationIdentity) -> NativeCredentialDaemon {
        NativeCredentialDaemon {
            backend: self.native.clone(),
            namespace: NativeCredentialNamespace::new(installation),
        }
    }

    #[cfg(any(feature = "native-credentials", test))]
    pub(super) fn with_backend(backend: Arc<dyn NativeCredentialStore>) -> Self {
        Self {
            native: Some(backend),
        }
    }

    #[cfg(test)]
    pub(super) fn from_backend<T>(backend: Arc<T>) -> Self
    where
        T: NativeCredentialStore + 'static,
    {
        Self::with_backend(backend)
    }
}

impl fmt::Debug for CredentialSourceCapabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialSourceCapabilities")
            .field("native_supported", &self.native_supported())
            .finish()
    }
}

#[derive(Clone)]
pub struct NativeCredentialManager {
    backend: Option<Arc<dyn NativeCredentialStore>>,
    namespace: NativeCredentialNamespace,
}

impl NativeCredentialManager {
    pub fn create(
        &self,
        name: &CredentialName,
        value: &SecretValue,
    ) -> Result<(), CredentialError> {
        self.write(name, value, |backend, locator| {
            backend.create(locator, value)
        })
    }

    pub fn set(&self, name: &CredentialName, value: &SecretValue) -> Result<(), CredentialError> {
        self.write(name, value, |backend, locator| backend.set(locator, value))
    }

    pub fn delete(&self, name: &CredentialName) -> Result<(), CredentialError> {
        self.call(name, |backend, locator| backend.delete(locator))
    }

    fn call(
        &self,
        name: &CredentialName,
        operation: impl FnOnce(
            &dyn NativeCredentialStore,
            &NativeCredentialLocator,
        ) -> Result<(), NativeStoreError>,
    ) -> Result<(), CredentialError> {
        let backend = self.backend.as_deref().ok_or_else(|| unsupported(name))?;
        let locator = self.namespace.locator(name);
        operation(backend, &locator).map_err(|error| map_backend_error(name, error))
    }

    fn write(
        &self,
        name: &CredentialName,
        value: &SecretValue,
        operation: impl FnOnce(
            &dyn NativeCredentialStore,
            &NativeCredentialLocator,
        ) -> Result<(), NativeStoreError>,
    ) -> Result<(), CredentialError> {
        let backend = self.backend.as_deref().ok_or_else(|| unsupported(name))?;
        validate_portable_native_credential(name, value)?;
        let locator = self.namespace.locator(name);
        operation(backend, &locator).map_err(|error| map_backend_error(name, error))
    }
}

impl fmt::Debug for NativeCredentialManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeCredentialManager")
            .field("native_supported", &self.backend.is_some())
            .field("namespace", &self.namespace)
            .finish()
    }
}

#[derive(Clone)]
pub struct NativeCredentialDaemon {
    backend: Option<Arc<dyn NativeCredentialStore>>,
    namespace: NativeCredentialNamespace,
}

impl NativeCredentialDaemon {
    pub fn read(&self, name: &CredentialName) -> Result<SecretValue, CredentialError> {
        let backend = self.backend.as_deref().ok_or_else(|| unsupported(name))?;
        let locator = self.namespace.locator(name);
        let value = backend
            .read(&locator)
            .map_err(|error| map_backend_error(name, error))?;
        validate_portable_native_credential(name, &value)?;
        Ok(value)
    }
}

impl fmt::Debug for NativeCredentialDaemon {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeCredentialDaemon")
            .field("native_supported", &self.backend.is_some())
            .field("namespace", &self.namespace)
            .finish()
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestNativeCredentialControl {
    backend: Arc<TestNativeCredentialStore>,
}

#[cfg(test)]
impl TestNativeCredentialControl {
    pub(crate) fn set_value(&self, value: SecretValue) {
        *self.backend.value.lock().expect("test native value lock") = Some(value);
    }

    pub(crate) fn set_missing(&self) {
        *self.backend.value.lock().expect("test native value lock") = None;
    }

    pub(crate) fn read_count(&self) -> usize {
        self.backend.reads.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
struct TestNativeCredentialStore {
    value: std::sync::Mutex<Option<SecretValue>>,
    reads: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl NativeCredentialStore for TestNativeCredentialStore {
    fn create(
        &self,
        _locator: &NativeCredentialLocator,
        _value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        Err(NativeStoreError::new(CredentialErrorCode::Unsupported))
    }

    fn set(
        &self,
        _locator: &NativeCredentialLocator,
        _value: &SecretValue,
    ) -> Result<(), NativeStoreError> {
        Err(NativeStoreError::new(CredentialErrorCode::Unsupported))
    }

    fn read(&self, _locator: &NativeCredentialLocator) -> Result<SecretValue, NativeStoreError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.value
            .lock()
            .expect("test native value lock")
            .clone()
            .ok_or_else(|| NativeStoreError::new(CredentialErrorCode::Missing))
    }

    fn delete(&self, _locator: &NativeCredentialLocator) -> Result<(), NativeStoreError> {
        Err(NativeStoreError::new(CredentialErrorCode::Unsupported))
    }
}

#[cfg(test)]
impl CredentialSourceCapabilities {
    pub(crate) fn test_native(value: SecretValue) -> (Self, TestNativeCredentialControl) {
        let backend = Arc::new(TestNativeCredentialStore {
            value: std::sync::Mutex::new(Some(value)),
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        (
            Self::with_backend(backend.clone()),
            TestNativeCredentialControl { backend },
        )
    }
}

fn unsupported(name: &CredentialName) -> CredentialError {
    CredentialError::new(
        CredentialErrorCode::Unsupported,
        CredentialSourceKind::Native,
        name.as_str(),
    )
}

fn validate_portable_native_credential(
    name: &CredentialName,
    value: &SecretValue,
) -> Result<(), CredentialError> {
    if value.expose().len() > NATIVE_CREDENTIAL_MAX_BYTES {
        return Err(CredentialError::new(
            CredentialErrorCode::Invalid,
            CredentialSourceKind::Native,
            name.as_str(),
        ));
    }
    Ok(())
}

fn map_backend_error(name: &CredentialName, error: NativeStoreError) -> CredentialError {
    CredentialError::new(error.code(), CredentialSourceKind::Native, name.as_str())
}
