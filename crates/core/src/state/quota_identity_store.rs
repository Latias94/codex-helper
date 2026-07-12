//! Installation-local key used for fallback quota-pool identity.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::file_replace::{
    AtomicWriteError, recover_uncertain_candidate, write_bytes_file_validated,
};

const MAGIC: &[u8; 8] = b"CHQKEY02";
const KEY_BYTES: usize = 32;
const CHECKSUM_BYTES: usize = 32;
const IDENTITY_BYTES: usize = MAGIC.len() + 8 + KEY_BYTES + CHECKSUM_BYTES;
const LOCK_WAIT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(5);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct InstallIdentity {
    key: [u8; KEY_BYTES],
    revision: u64,
}

impl fmt::Debug for InstallIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InstallIdentity")
            .field("revision", &self.revision)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl InstallIdentity {
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityLoadOutcome {
    Loaded,
    Created,
    ReplacedCorrupt,
}

#[derive(Debug, Clone)]
pub struct QuotaIdentityStore {
    path: PathBuf,
}

impl QuotaIdentityStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_create(&self) -> io::Result<(InstallIdentity, IdentityLoadOutcome)> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = IdentityFileLock::acquire(lock_path_for(&self.path))?;
        reject_reparse_point(&self.path)?;
        if self.path.exists() {
            return load_existing_identity(&self.path, read_identity);
        }

        let identity = new_identity(0);
        match write_identity_create_new(&self.path, &identity) {
            Ok(()) => {
                enforce_private_permissions(&self.path)?;
                Ok((identity, IdentityLoadOutcome::Created))
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                load_existing_identity(&self.path, read_identity)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug)]
enum IdentityReadError {
    Io(io::Error),
    Corrupt(io::Error),
}

fn load_existing_identity(
    path: &Path,
    read: impl FnOnce(&Path) -> Result<InstallIdentity, IdentityReadError>,
) -> io::Result<(InstallIdentity, IdentityLoadOutcome)> {
    match read(path) {
        Ok(identity) => {
            enforce_private_permissions(path)?;
            Ok((identity, IdentityLoadOutcome::Loaded))
        }
        Err(IdentityReadError::Corrupt(_error)) => {
            let previous_revision = read_revision_hint(path).unwrap_or(0);
            let identity = new_identity(previous_revision.saturating_add(1));
            replace_identity_atomically(path, &identity)?;
            enforce_private_permissions(path)?;
            Ok((identity, IdentityLoadOutcome::ReplacedCorrupt))
        }
        Err(IdentityReadError::Io(error)) => Err(error),
    }
}

fn new_identity(revision: u64) -> InstallIdentity {
    let first = *Uuid::new_v4().as_bytes();
    let second = *Uuid::new_v4().as_bytes();
    let mut key = [0_u8; KEY_BYTES];
    key[..16].copy_from_slice(&first);
    key[16..].copy_from_slice(&second);
    InstallIdentity { key, revision }
}

fn read_identity(path: &Path) -> Result<InstallIdentity, IdentityReadError> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(IdentityReadError::Io)?
        .read_to_end(&mut bytes)
        .map_err(IdentityReadError::Io)?;
    decode_identity(&bytes).map_err(IdentityReadError::Corrupt)
}

fn decode_identity(bytes: &[u8]) -> io::Result<InstallIdentity> {
    if bytes.len() != IDENTITY_BYTES || &bytes[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid quota identity header",
        ));
    }
    let payload_end = MAGIC.len() + 8 + KEY_BYTES;
    let expected: [u8; CHECKSUM_BYTES] = Sha256::digest(&bytes[..payload_end]).into();
    if bytes[payload_end..] != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "quota identity checksum mismatch",
        ));
    }
    let revision_offset = MAGIC.len();
    let revision = u64::from_le_bytes(
        bytes[revision_offset..revision_offset + 8]
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid revision"))?,
    );
    let key_offset = revision_offset + 8;
    let mut key = [0_u8; KEY_BYTES];
    key.copy_from_slice(&bytes[key_offset..key_offset + KEY_BYTES]);
    if key.iter().all(|byte| *byte == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty quota identity key",
        ));
    }
    Ok(InstallIdentity { key, revision })
}

fn read_revision_hint(path: &Path) -> Option<u64> {
    let mut bytes = [0_u8; 16];
    let mut file = fs::File::open(path).ok()?;
    file.read_exact(&mut bytes).ok()?;
    if &bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    Some(u64::from_le_bytes(bytes[MAGIC.len()..].try_into().ok()?))
}

fn identity_bytes(identity: &InstallIdentity) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(IDENTITY_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&identity.revision.to_le_bytes());
    bytes.extend_from_slice(&identity.key);
    let checksum: [u8; CHECKSUM_BYTES] = Sha256::digest(&bytes).into();
    bytes.extend_from_slice(&checksum);
    bytes
}

fn validate_identity_bytes(bytes: &[u8]) -> io::Result<()> {
    decode_identity(bytes).map(|_| ())
}

fn write_identity_create_new(path: &Path, identity: &InstallIdentity) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&identity_bytes(identity))?;
    file.sync_all()
}

fn replace_identity_atomically(path: &Path, identity: &InstallIdentity) -> io::Result<()> {
    let candidate = identity_bytes(identity);
    match write_bytes_file_validated(path, &candidate, validate_identity_bytes) {
        Ok(()) => Ok(()),
        Err(error @ AtomicWriteError::BeforeCommit { .. }) => Err(io::Error::other(error)),
        Err(error @ AtomicWriteError::CommitStateUnknown { .. }) => {
            recover_uncertain_candidate(path, &candidate, error, validate_identity_bytes)
        }
    }
}

fn lock_path_for(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("quota-identity.key");
    path.with_file_name(format!(".{name}.lock"))
}

struct IdentityFileLock {
    path: PathBuf,
    token: String,
}

impl IdentityFileLock {
    fn acquire(path: PathBuf) -> io::Result<Self> {
        let token = format!("{}:{}", std::process::id(), Uuid::new_v4());
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(token.as_bytes())?;
                    file.sync_all()?;
                    return Ok(Self { path, token });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= LOCK_WAIT {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out acquiring quota identity lock",
                        ));
                    }
                    thread::sleep(LOCK_RETRY);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for IdentityFileLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).ok().as_deref() == Some(self.token.as_str()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= STALE_LOCK_AGE)
}

#[cfg(unix)]
fn enforce_private_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "quota identity permissions are not owner-only",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn enforce_private_permissions(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SetFileSecurityW,
    };

    let path_wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    // Owner, LocalSystem and Administrators retain full access; inheritance and broad groups are
    // removed. Failure is fatal so an identity key is never accepted with unknown ACL posture.
    let sddl = "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: SDDL/path buffers are null terminated and descriptor is released with LocalFree.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let applied = unsafe {
        SetFileSecurityW(
            path_wide.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe {
        LocalFree(descriptor.cast());
    }
    if applied == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn enforce_private_permissions(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "cannot guarantee quota identity file permissions on this platform",
    ))
}

#[cfg(unix)]
fn reject_reparse_point(path: &Path) -> io::Result<()> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "quota identity path must not be a symlink",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn reject_reparse_point(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    if path.exists()
        && fs::symlink_metadata(path)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "quota identity path must not be a reparse point",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn reject_reparse_point(path: &Path) -> io::Result<()> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "quota identity path must not be a symlink",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("codex-helper-quota-key-{}", Uuid::new_v4()))
    }

    #[test]
    fn identity_is_stable_checksum_valid_and_redacted() {
        let path = temp_path();
        let store = QuotaIdentityStore::new(&path);
        let (first, first_outcome) = store.load_or_create().expect("create identity");
        let (second, second_outcome) = store.load_or_create().expect("load identity");
        assert_eq!(first_outcome, IdentityLoadOutcome::Created);
        assert_eq!(second_outcome, IdentityLoadOutcome::Loaded);
        assert_eq!(first.key(), second.key());
        assert_eq!(
            decode_identity(&fs::read(&path).expect("identity bytes"))
                .expect("checksum")
                .key(),
            first.key()
        );
        assert!(!format!("{first:?}").contains(&format!("{:x?}", first.key())));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn checksum_corruption_rotates_revision_and_key() {
        let path = temp_path();
        let store = QuotaIdentityStore::new(&path);
        let (first, _) = store.load_or_create().expect("create identity");
        let mut bytes = fs::read(&path).expect("identity bytes");
        bytes[20] ^= 0xff;
        fs::write(&path, bytes).expect("corrupt checksum");
        let (second, outcome) = store.load_or_create().expect("replace identity");
        assert_eq!(outcome, IdentityLoadOutcome::ReplacedCorrupt);
        assert_ne!(first.key(), second.key());
        assert_eq!(second.revision(), 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn transient_read_errors_do_not_rotate_identity() {
        let path = temp_path();
        let store = QuotaIdentityStore::new(&path);
        let (first, _) = store.load_or_create().expect("create identity");
        let original_bytes = fs::read(&path).expect("identity bytes");

        let assert_preserved = |read_error: io::Error| {
            let expected_kind = read_error.kind();
            let error = load_existing_identity(&path, |_| Err(IdentityReadError::Io(read_error)))
                .expect_err("transient read error must be returned");
            assert_eq!(error.kind(), expected_kind);
            assert_eq!(
                fs::read(&path).expect("identity remains readable"),
                original_bytes
            );
        };
        assert_preserved(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "permission denied",
        ));
        assert_preserved(io::Error::new(
            io::ErrorKind::WouldBlock,
            "temporarily unavailable",
        ));
        #[cfg(windows)]
        assert_preserved(io::Error::from_raw_os_error(32));

        let (loaded, outcome) = store.load_or_create().expect("reload identity");
        assert_eq!(outcome, IdentityLoadOutcome::Loaded);
        assert_eq!(loaded.key(), first.key());
        assert_eq!(loaded.revision(), first.revision());
        let _ = fs::remove_file(path);
    }

    #[cfg(windows)]
    #[test]
    fn sharing_violation_does_not_rotate_identity() {
        use std::os::windows::fs::OpenOptionsExt;

        let path = temp_path();
        let store = QuotaIdentityStore::new(&path);
        let (first, _) = store.load_or_create().expect("create identity");
        let original_bytes = fs::read(&path).expect("identity bytes");
        let exclusive_reader = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("open identity without sharing");

        let error = load_existing_identity(&path, read_identity)
            .expect_err("concurrent exclusive access must fail");
        assert_eq!(error.raw_os_error(), Some(32));
        drop(exclusive_reader);

        assert_eq!(
            fs::read(&path).expect("identity remains readable"),
            original_bytes
        );
        let (loaded, outcome) = store.load_or_create().expect("reload identity");
        assert_eq!(outcome, IdentityLoadOutcome::Loaded);
        assert_eq!(loaded.key(), first.key());
        assert_eq!(loaded.revision(), first.revision());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn concurrent_creators_share_one_identity() {
        let path = temp_path();
        let handles = (0..2)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    QuotaIdentityStore::new(path)
                        .load_or_create()
                        .expect("identity")
                        .0
                })
            })
            .collect::<Vec<_>>();
        let first = handles[0].thread().id();
        let identities = handles
            .into_iter()
            .map(|handle| handle.join().expect("join"))
            .collect::<Vec<_>>();
        let _ = first;
        assert_eq!(identities[0].key(), identities[1].key());
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn identity_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path();
        QuotaIdentityStore::new(&path)
            .load_or_create()
            .expect("create identity");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_file(path);
    }

    #[cfg(windows)]
    #[test]
    fn identity_file_acl_hardening_must_succeed_on_windows() {
        let path = temp_path();
        QuotaIdentityStore::new(&path)
            .load_or_create()
            .expect("secure identity");
        enforce_private_permissions(&path).expect("ACL remains enforceable");
        let _ = fs::remove_file(path);
    }
}
