use std::fmt;
use std::fmt::Write as _;

#[cfg(any(feature = "native-credentials", test))]
use std::collections::HashMap;
#[cfg(any(feature = "native-credentials", test))]
use std::fs::{File, OpenOptions};
#[cfg(any(feature = "native-credentials", test))]
use std::path::{Path, PathBuf};
#[cfg(any(feature = "native-credentials", test))]
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::installation_identity::InstallationIdentity;
use super::model::CredentialName;

#[cfg(all(feature = "native-credentials", target_os = "linux"))]
mod linux;
#[cfg(all(feature = "native-credentials", target_os = "macos"))]
mod macos;
#[cfg(all(feature = "native-credentials", windows))]
mod windows;

const LOCATOR_DOMAIN: &[u8] = b"codex-helper/native-credential-locator/v1\0";

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct NativeCredentialLocator(String);

impl NativeCredentialLocator {
    #[cfg(any(feature = "native-credentials", test))]
    pub(super) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for NativeCredentialLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeCredentialLocator(<opaque>)")
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct NativeCredentialNamespace {
    installation: InstallationIdentity,
}

impl NativeCredentialNamespace {
    pub(super) fn new(installation: InstallationIdentity) -> Self {
        Self { installation }
    }

    pub(super) fn locator(&self, name: &CredentialName) -> NativeCredentialLocator {
        let mut digest = Sha256::new();
        digest.update(LOCATOR_DOMAIN);
        digest.update(self.installation.uuid().as_bytes());
        digest.update(name.as_str().as_bytes());
        NativeCredentialLocator(format!(
            "codex-helper:v1:{}",
            lowercase_hex(&digest.finalize())
        ))
    }
}

fn lowercase_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[cfg(any(feature = "native-credentials", test))]
pub(super) struct LocatorLocks {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    directory: PathBuf,
}

#[cfg(any(feature = "native-credentials", test))]
impl Default for LocatorLocks {
    fn default() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            directory: native_lock_directory(),
        }
    }
}

#[cfg(any(feature = "native-credentials", test))]
impl LocatorLocks {
    #[cfg(test)]
    fn in_directory(directory: PathBuf) -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            directory,
        }
    }

    pub(super) fn with_lock<T>(
        &self,
        locator: &NativeCredentialLocator,
        operation: impl FnOnce() -> Result<T, super::capabilities::NativeStoreError>,
    ) -> Result<T, super::capabilities::NativeStoreError> {
        let lock = self
            .locks
            .lock()
            .map_err(|_| backend_unavailable())?
            .entry(locator.as_str().to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().map_err(|_| backend_unavailable())?;
        let _process_guard = lock_locator_file(locator, &self.directory)?;
        operation()
    }
}

#[cfg(any(feature = "native-credentials", test))]
fn lock_locator_file(
    locator: &NativeCredentialLocator,
    directory: &Path,
) -> Result<File, super::capabilities::NativeStoreError> {
    prepare_native_lock_directory(directory)?;

    let digest = Sha256::digest(locator.as_str().as_bytes());
    let path = directory.join(format!("{}.lock", lowercase_hex(&digest)));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(&path).map_err(|_| backend_unavailable())?;
    validate_native_lock_file(&file, &path)?;
    file.lock().map_err(|_| backend_unavailable())?;
    Ok(file)
}

#[cfg(any(feature = "native-credentials", test))]
fn prepare_native_lock_directory(
    directory: &Path,
) -> Result<(), super::capabilities::NativeStoreError> {
    if let Some(parent) = directory.parent() {
        prepare_private_directory(parent)?;
    }
    prepare_private_directory(directory)
}

#[cfg(any(feature = "native-credentials", test))]
fn prepare_private_directory(
    directory: &Path,
) -> Result<(), super::capabilities::NativeStoreError> {
    std::fs::create_dir_all(directory).map_err(|_| backend_unavailable())?;
    let metadata = std::fs::symlink_metadata(directory).map_err(|_| backend_unavailable())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(backend_unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(backend_unavailable());
        }
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| backend_unavailable())?;
    }
    Ok(())
}

#[cfg(any(feature = "native-credentials", test))]
fn validate_native_lock_file(
    file: &File,
    path: &Path,
) -> Result<(), super::capabilities::NativeStoreError> {
    let metadata = file.metadata().map_err(|_| backend_unavailable())?;
    if !metadata.is_file() {
        return Err(backend_unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(backend_unavailable());
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| backend_unavailable())?;
    }
    Ok(())
}

#[cfg(any(feature = "native-credentials", test))]
fn native_lock_directory() -> PathBuf {
    crate::config::proxy_home_dir()
        .join("state")
        .join("native-credential-locks")
}

#[cfg(any(feature = "native-credentials", test))]
fn backend_unavailable() -> super::capabilities::NativeStoreError {
    super::capabilities::NativeStoreError::new(
        super::capabilities::NativeStoreErrorCode::BackendUnavailable,
    )
}

#[cfg(all(feature = "native-credentials", target_os = "linux"))]
pub(super) fn platform_store() -> std::sync::Arc<dyn super::capabilities::NativeCredentialStore> {
    std::sync::Arc::new(linux::LinuxNativeCredentialStore::new())
}

#[cfg(all(feature = "native-credentials", target_os = "macos"))]
pub(super) fn platform_store() -> std::sync::Arc<dyn super::capabilities::NativeCredentialStore> {
    std::sync::Arc::new(macos::MacosNativeCredentialStore::new())
}

#[cfg(all(feature = "native-credentials", windows))]
pub(super) fn platform_store() -> std::sync::Arc<dyn super::capabilities::NativeCredentialStore> {
    std::sync::Arc::new(windows::WindowsNativeCredentialStore::new())
}

#[cfg(all(
    feature = "native-credentials",
    not(any(target_os = "linux", target_os = "macos", windows))
))]
pub(super) fn platform_store() -> std::sync::Arc<dyn super::capabilities::NativeCredentialStore> {
    std::sync::Arc::new(UnsupportedNativeCredentialStore)
}

#[cfg(all(
    feature = "native-credentials",
    not(any(target_os = "linux", target_os = "macos", windows))
))]
struct UnsupportedNativeCredentialStore;

#[cfg(all(
    feature = "native-credentials",
    not(any(target_os = "linux", target_os = "macos", windows))
))]
impl super::capabilities::NativeCredentialStore for UnsupportedNativeCredentialStore {
    fn create(
        &self,
        _locator: &NativeCredentialLocator,
        _value: &super::model::SecretValue,
    ) -> Result<(), super::capabilities::NativeStoreError> {
        Err(super::capabilities::NativeStoreError::new(
            super::capabilities::NativeStoreErrorCode::Unsupported,
        ))
    }

    fn set(
        &self,
        _locator: &NativeCredentialLocator,
        _value: &super::model::SecretValue,
    ) -> Result<(), super::capabilities::NativeStoreError> {
        Err(super::capabilities::NativeStoreError::new(
            super::capabilities::NativeStoreErrorCode::Unsupported,
        ))
    }

    fn read(
        &self,
        _locator: &NativeCredentialLocator,
    ) -> Result<super::model::SecretValue, super::capabilities::NativeStoreError> {
        Err(super::capabilities::NativeStoreError::new(
            super::capabilities::NativeStoreErrorCode::Unsupported,
        ))
    }

    fn delete(
        &self,
        _locator: &NativeCredentialLocator,
    ) -> Result<(), super::capabilities::NativeStoreError> {
        Err(super::capabilities::NativeStoreError::new(
            super::capabilities::NativeStoreErrorCode::Unsupported,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn locator_lock_serializes_independent_store_instances_for_one_native_entry() {
        let root = std::env::temp_dir().join(format!(
            "codex-helper-native-lock-test-{}",
            uuid::Uuid::new_v4()
        ));
        let directory = root.join("state").join("native-credential-locks");
        let locks = [
            Arc::new(LocatorLocks::in_directory(directory.clone())),
            Arc::new(LocatorLocks::in_directory(directory.clone())),
        ];
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let locator = NativeCredentialLocator("test-locator".to_string());
        let mut workers = Vec::new();

        for index in 0..8 {
            let locks = Arc::clone(&locks[index % locks.len()]);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let locator = locator.clone();
            workers.push(thread::spawn(move || {
                locks
                    .with_lock(&locator, || {
                        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                        maximum.fetch_max(current, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(5));
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .expect("serialized locator operation");
            }));
        }

        for worker in workers {
            worker.join().expect("locator worker");
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(root).expect("remove locator lock test directory");
    }

    #[cfg(unix)]
    #[test]
    fn locator_lock_rejects_a_symlinked_lock_directory() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "codex-helper-native-lock-symlink-test-{}",
            uuid::Uuid::new_v4()
        ));
        let target = root.join("target");
        let directory = root.join("state").join("native-credential-locks");
        std::fs::create_dir_all(&target).expect("create symlink target");
        std::fs::create_dir_all(directory.parent().expect("lock directory parent"))
            .expect("create lock directory parent");
        symlink(&target, &directory).expect("create lock directory symlink");
        let locks = LocatorLocks::in_directory(directory);
        let locator = NativeCredentialLocator("test-locator".to_string());

        let error = locks
            .with_lock(&locator, || Ok(()))
            .expect_err("symlinked lock directory must fail closed");

        assert_eq!(
            error.code(),
            super::super::capabilities::NativeStoreErrorCode::BackendUnavailable
        );
        std::fs::remove_dir_all(root).expect("remove symlink lock test directory");
    }

    #[test]
    fn locator_is_lowercase_and_case_fold_collision_safe() {
        let namespace = NativeCredentialNamespace::new(InstallationIdentity::from_uuid(
            uuid::Uuid::from_bytes([7; 16]),
        ));
        let locator = namespace
            .locator(&CredentialName::parse("relay.primary").expect("valid credential name"));
        assert_eq!(locator.as_str(), locator.as_str().to_ascii_lowercase());
        assert!(
            locator
                .as_str()
                .strip_prefix("codex-helper:v1:")
                .is_some_and(|digest| digest.len() == 64
                    && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        );
    }
}
