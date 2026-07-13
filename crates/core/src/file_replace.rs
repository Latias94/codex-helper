use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use thiserror::Error;

const TEMP_FILE_PREFIX: &str = ".codex-helper-tmp-v1-";
const TEMP_FILE_CREATE_ATTEMPTS: usize = 16;
const STALE_TEMP_FILE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
#[cfg(windows)]
const WINDOWS_REPLACE_ATTEMPTS: usize = 10;
#[cfg(windows)]
const WINDOWS_REPLACE_MAX_BACKOFF: Duration = Duration::from_millis(16);

#[derive(Debug, Error)]
pub(crate) enum AtomicWriteError {
    #[error("atomic write to {path:?} failed before commit during {stage}: {source}")]
    BeforeCommit {
        path: PathBuf,
        stage: &'static str,
        #[source]
        source: io::Error,
    },
    #[error(
        "atomic write to {path:?} may have committed before {stage} reported an error: {source}"
    )]
    CommitStateUnknown {
        path: PathBuf,
        stage: &'static str,
        #[source]
        source: io::Error,
    },
}

impl AtomicWriteError {
    fn before_commit(path: &Path, stage: &'static str, source: io::Error) -> Self {
        Self::BeforeCommit {
            path: path.to_path_buf(),
            stage,
            source,
        }
    }

    fn commit_state_unknown(path: &Path, stage: &'static str, source: io::Error) -> Self {
        Self::CommitStateUnknown {
            path: path.to_path_buf(),
            stage,
            source,
        }
    }
}

struct StagedFileGuard {
    path: PathBuf,
    armed: bool,
}

impl StagedFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagedFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn destination_parent(path: &Path) -> io::Result<PathBuf> {
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination must name a file",
        ));
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if parent.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(parent.to_path_buf())
    }
}

fn create_staged_file(parent: &Path) -> io::Result<(File, StagedFileGuard)> {
    for _ in 0..TEMP_FILE_CREATE_ATTEMPTS {
        let path = parent.join(format!(
            "{TEMP_FILE_PREFIX}{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, StagedFileGuard::new(path))),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique staging file",
    ))
}

fn is_managed_temp_file_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(suffix) = name.strip_prefix(TEMP_FILE_PREFIX) else {
        return false;
    };
    let Some((process_id, uuid)) = suffix.split_once('-') else {
        return false;
    };

    !process_id.is_empty()
        && process_id.bytes().all(|byte| byte.is_ascii_digit())
        && uuid::Uuid::parse_str(uuid).is_ok()
}

fn prune_stale_temp_files(parent: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };

    for entry in entries.flatten() {
        if !is_managed_temp_file_name(&entry.file_name()) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age >= STALE_TEMP_FILE_AGE {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn flush_and_sync_staged_file(file: &mut File) -> io::Result<()> {
    file.flush()?;
    file.sync_all()
}

fn write_staged_file(file: &mut File, data: &[u8]) -> io::Result<()> {
    file.write_all(data)
}

fn preserve_destination_permissions(destination: &Path, staged_file: &File) -> io::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_file() => {
            staged_file.set_permissions(metadata.permissions())
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn apply_staged_permissions(
    destination: &Path,
    staged_file: &File,
    explicit_permissions: Option<fs::Permissions>,
) -> io::Result<()> {
    match explicit_permissions {
        Some(permissions) => staged_file.set_permissions(permissions),
        None => preserve_destination_permissions(destination, staged_file),
    }
}

fn before_replace_noop(_staged_path: &Path, _destination: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn replace_existing_file(staged_path: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if path[..path.len().saturating_sub(1)].contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains an embedded null",
            ));
        }
        Ok(path)
    }

    let staged_path = wide_path(staged_path)?;
    let destination = wide_path(destination)?;
    let mut backoff = Duration::from_millis(1);
    for attempt in 0..WINDOWS_REPLACE_ATTEMPTS {
        // ReplaceFileW has partial-failure modes that can temporarily remove the destination.
        // MoveFileExW provides the single rename operation required for old-or-new visibility.
        // SAFETY: Both buffers are null-terminated and remain alive for the duration of the call.
        let replaced = unsafe {
            MoveFileExW(
                staged_path.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if replaced != 0 {
            return Ok(());
        }

        let err = io::Error::last_os_error();
        let retryable = matches!(
            err.raw_os_error(),
            Some(code)
                if code == ERROR_ACCESS_DENIED as i32
                    || code == ERROR_SHARING_VIOLATION as i32
                    || code == ERROR_LOCK_VIOLATION as i32
        );
        if !retryable || attempt + 1 == WINDOWS_REPLACE_ATTEMPTS {
            return Err(err);
        }

        std::thread::sleep(backoff);
        backoff = std::cmp::min(backoff.saturating_mul(2), WINDOWS_REPLACE_MAX_BACKOFF);
    }

    unreachable!("the bounded replacement loop always returns on its final attempt")
}

#[cfg(not(windows))]
fn replace_existing_file(staged_path: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(staged_path, destination)
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    // MOVEFILE_WRITE_THROUGH above is the strongest directory-entry durability primitive
    // available without opening a directory handle with Windows-specific flags.
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    match File::open(parent).and_then(|directory| directory.sync_all()) {
        Ok(()) => Ok(()),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::InvalidInput
                    | io::ErrorKind::PermissionDenied
                    | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(err) => Err(err),
    }
}

struct AtomicWriteOperations<W, S, B, R, D> {
    write_staged: W,
    sync_staged: S,
    before_replace: B,
    replace: R,
    sync_parent: D,
}

fn write_bytes_file_with_operations<V, W, S, B, R, D>(
    path: &Path,
    data: &[u8],
    explicit_permissions: Option<fs::Permissions>,
    validate: V,
    operations: AtomicWriteOperations<W, S, B, R, D>,
) -> std::result::Result<(), AtomicWriteError>
where
    V: FnOnce(&[u8]) -> io::Result<()>,
    W: FnOnce(&mut File, &[u8]) -> io::Result<()>,
    S: FnOnce(&mut File) -> io::Result<()>,
    B: FnOnce(&Path, &Path) -> io::Result<()>,
    R: FnOnce(&Path, &Path) -> io::Result<()>,
    D: FnOnce(&Path) -> io::Result<()>,
{
    let AtomicWriteOperations {
        write_staged,
        sync_staged,
        before_replace,
        replace,
        sync_parent,
    } = operations;
    let parent = destination_parent(path)
        .map_err(|err| AtomicWriteError::before_commit(path, "resolve parent", err))?;
    fs::create_dir_all(&parent)
        .map_err(|err| AtomicWriteError::before_commit(path, "create parent", err))?;
    prune_stale_temp_files(&parent, SystemTime::now());

    let (mut staged_file, mut staged_guard) = create_staged_file(&parent)
        .map_err(|err| AtomicWriteError::before_commit(path, "create staging file", err))?;
    apply_staged_permissions(path, &staged_file, explicit_permissions)
        .map_err(|err| AtomicWriteError::before_commit(path, "apply permissions", err))?;
    write_staged(&mut staged_file, data)
        .map_err(|err| AtomicWriteError::before_commit(path, "write staging file", err))?;
    sync_staged(&mut staged_file)
        .map_err(|err| AtomicWriteError::before_commit(path, "sync staging file", err))?;
    drop(staged_file);

    let staged_bytes = fs::read(staged_guard.path())
        .map_err(|err| AtomicWriteError::before_commit(path, "read staging file", err))?;
    if staged_bytes != data {
        return Err(AtomicWriteError::before_commit(
            path,
            "verify staging bytes",
            io::Error::new(
                io::ErrorKind::InvalidData,
                "staging bytes differ from the requested payload",
            ),
        ));
    }
    validate(&staged_bytes)
        .map_err(|err| AtomicWriteError::before_commit(path, "validate staging file", err))?;
    before_replace(staged_guard.path(), path)
        .map_err(|err| AtomicWriteError::before_commit(path, "before replace", err))?;

    if let Err(err) = replace(staged_guard.path(), path) {
        let staged_file_still_exists = matches!(staged_guard.path().try_exists(), Ok(true));
        return Err(if staged_file_still_exists {
            AtomicWriteError::before_commit(path, "replace destination", err)
        } else {
            AtomicWriteError::commit_state_unknown(path, "replace destination", err)
        });
    }
    staged_guard.disarm();

    sync_parent(&parent)
        .map_err(|err| AtomicWriteError::commit_state_unknown(path, "sync parent", err))?;
    Ok(())
}

pub(crate) fn write_bytes_file(
    path: &Path,
    data: &[u8],
) -> std::result::Result<(), AtomicWriteError> {
    write_bytes_file_validated(path, data, |_| Ok(()))
}

pub(crate) fn write_bytes_file_validated<V>(
    path: &Path,
    data: &[u8],
    validate: V,
) -> std::result::Result<(), AtomicWriteError>
where
    V: FnOnce(&[u8]) -> io::Result<()>,
{
    write_bytes_file_validated_with_permissions(path, data, None, validate)
}

fn write_bytes_file_validated_with_permissions<V>(
    path: &Path,
    data: &[u8],
    permissions: Option<fs::Permissions>,
    validate: V,
) -> std::result::Result<(), AtomicWriteError>
where
    V: FnOnce(&[u8]) -> io::Result<()>,
{
    write_bytes_file_with_operations(
        path,
        data,
        permissions,
        validate,
        AtomicWriteOperations {
            write_staged: write_staged_file,
            sync_staged: flush_and_sync_staged_file,
            before_replace: before_replace_noop,
            replace: replace_existing_file,
            sync_parent: sync_parent_directory,
        },
    )
}

pub fn write_text_file(path: &Path, data: &str) -> Result<()> {
    write_bytes_file(path, data.as_bytes()).with_context(|| format!("atomically write {:?}", path))
}

pub async fn write_bytes_file_async(
    path: &Path,
    data: &[u8],
) -> std::result::Result<(), AtomicWriteError> {
    write_bytes_file_validated_async(path, data, |_| Ok(())).await
}

pub(crate) async fn write_bytes_file_async_with_permissions(
    path: &Path,
    data: &[u8],
    permissions: fs::Permissions,
) -> std::result::Result<(), AtomicWriteError> {
    let path = path.to_path_buf();
    let error_path = path.clone();
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || {
        write_bytes_file_validated_with_permissions(&path, &data, Some(permissions), |_| Ok(()))
    })
    .await
    .map_err(|err| {
        AtomicWriteError::commit_state_unknown(
            &error_path,
            "join blocking writer",
            io::Error::other(err),
        )
    })?
}

pub(crate) async fn write_bytes_file_validated_async<V>(
    path: &Path,
    data: &[u8],
    validate: V,
) -> std::result::Result<(), AtomicWriteError>
where
    V: FnOnce(&[u8]) -> io::Result<()> + Send + 'static,
{
    let path = path.to_path_buf();
    let error_path = path.clone();
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || write_bytes_file_validated(&path, &data, validate))
        .await
        .map_err(|err| {
            AtomicWriteError::commit_state_unknown(
                &error_path,
                "join blocking writer",
                io::Error::other(err),
            )
        })?
}

#[cfg(test)]
mod tests {
    use std::fs::FileTimes;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-helper-file-replace-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    let _ = fs::remove_file(entry.path());
                }
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    fn managed_temp_files(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .expect("read test directory")
            .flatten()
            .filter(|entry| is_managed_temp_file_name(&entry.file_name()))
            .map(|entry| entry.path())
            .collect()
    }

    fn fail_write(_file: &mut File, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::other("injected staging write failure"))
    }

    #[cfg(unix)]
    fn require_private_mode_before_write(file: &mut File, data: &[u8]) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let mode = file.metadata()?.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("staging mode was {mode:o}, expected 600"),
            ));
        }
        file.write_all(data)
    }

    fn fail_sync(_file: &mut File) -> io::Result<()> {
        Err(io::Error::other("injected staging sync failure"))
    }

    fn fail_before_replace(_staged_path: &Path, _destination: &Path) -> io::Result<()> {
        Err(io::Error::other("injected pre-replace failure"))
    }

    fn replace_then_report_error(staged_path: &Path, destination: &Path) -> io::Result<()> {
        replace_existing_file(staged_path, destination)?;
        Err(io::Error::other("injected post-replace uncertainty"))
    }

    fn fail_replace_without_commit(_staged_path: &Path, _destination: &Path) -> io::Result<()> {
        Err(io::Error::other("injected replacement failure"))
    }

    fn fail_parent_sync(_parent: &Path) -> io::Result<()> {
        Err(io::Error::other("injected parent sync failure"))
    }

    #[test]
    fn write_text_file_overwrites_existing_file_and_cleans_staging_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, "old").expect("write old file");

        write_text_file(&path, "new").expect("overwrite file");

        assert_eq!(fs::read_to_string(&path).expect("read new file"), "new");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_existing_unix_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640))
            .expect("set destination mode");

        write_bytes_file(&path, b"new").expect("replace destination");

        let mode = fs::metadata(&path)
            .expect("read destination mode")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_permissions_are_applied_before_sensitive_bytes_are_written() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDir::new();
        let path = directory.join("backup.toml");
        write_bytes_file_with_operations(
            &path,
            b"token = 'secret'",
            Some(fs::Permissions::from_mode(0o600)),
            |_| Ok(()),
            AtomicWriteOperations {
                write_staged: require_private_mode_before_write,
                sync_staged: flush_and_sync_staged_file,
                before_replace: before_replace_noop,
                replace: replace_existing_file,
                sync_parent: sync_parent_directory,
            },
        )
        .expect("write sensitive bytes only after applying private mode");

        let mode = fs::metadata(&path)
            .expect("read destination mode")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[tokio::test]
    async fn write_bytes_file_async_overwrites_existing_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        write_bytes_file_async(&path, b"new")
            .await
            .expect("overwrite file");

        assert_eq!(fs::read(&path).expect("read new file"), b"new");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn staging_write_failure_preserves_old_destination_and_cleans_staging_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        let err = write_bytes_file_with_operations(
            &path,
            b"new",
            None,
            |_| Ok(()),
            AtomicWriteOperations {
                write_staged: fail_write,
                sync_staged: flush_and_sync_staged_file,
                before_replace: before_replace_noop,
                replace: replace_existing_file,
                sync_parent: sync_parent_directory,
            },
        )
        .expect_err("write failure should be reported");

        assert!(matches!(err, AtomicWriteError::BeforeCommit { .. }));
        assert_eq!(fs::read(&path).expect("read old file"), b"old");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn staging_sync_failure_preserves_old_destination_and_cleans_staging_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        let err = write_bytes_file_with_operations(
            &path,
            b"new",
            None,
            |_| Ok(()),
            AtomicWriteOperations {
                write_staged: write_staged_file,
                sync_staged: fail_sync,
                before_replace: before_replace_noop,
                replace: replace_existing_file,
                sync_parent: sync_parent_directory,
            },
        )
        .expect_err("sync failure should be reported");

        assert!(matches!(err, AtomicWriteError::BeforeCommit { .. }));
        assert_eq!(fs::read(&path).expect("read old file"), b"old");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn validation_failure_preserves_old_destination_and_cleans_staging_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        let err = write_bytes_file_validated(&path, b"new", |_| {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "injected validation failure",
            ))
        })
        .expect_err("validation failure should be reported");

        assert!(matches!(err, AtomicWriteError::BeforeCommit { .. }));
        assert_eq!(fs::read(&path).expect("read old file"), b"old");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn pre_replace_failure_preserves_old_destination_and_cleans_staging_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        let err = write_bytes_file_with_operations(
            &path,
            b"new",
            None,
            |_| Ok(()),
            AtomicWriteOperations {
                write_staged: write_staged_file,
                sync_staged: flush_and_sync_staged_file,
                before_replace: fail_before_replace,
                replace: replace_existing_file,
                sync_parent: sync_parent_directory,
            },
        )
        .expect_err("pre-replace failure should be reported");

        assert!(matches!(err, AtomicWriteError::BeforeCommit { .. }));
        assert_eq!(fs::read(&path).expect("read old file"), b"old");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn failed_replace_with_staging_present_is_not_committed() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        let err = write_bytes_file_with_operations(
            &path,
            b"new",
            None,
            |_| Ok(()),
            AtomicWriteOperations {
                write_staged: write_staged_file,
                sync_staged: flush_and_sync_staged_file,
                before_replace: before_replace_noop,
                replace: fail_replace_without_commit,
                sync_parent: sync_parent_directory,
            },
        )
        .expect_err("replacement failure should be reported");

        assert!(matches!(err, AtomicWriteError::BeforeCommit { .. }));
        assert_eq!(fs::read(&path).expect("read old file"), b"old");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn post_replace_uncertainty_exposes_complete_new_destination() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        let err = write_bytes_file_with_operations(
            &path,
            b"new",
            None,
            |_| Ok(()),
            AtomicWriteOperations {
                write_staged: write_staged_file,
                sync_staged: flush_and_sync_staged_file,
                before_replace: before_replace_noop,
                replace: replace_then_report_error,
                sync_parent: sync_parent_directory,
            },
        )
        .expect_err("post-replace uncertainty should be reported");

        assert!(matches!(err, AtomicWriteError::CommitStateUnknown { .. }));
        assert_eq!(fs::read(&path).expect("read recovered file"), b"new");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn parent_sync_failure_is_reported_as_maybe_committed() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, b"old").expect("write old file");

        let err = write_bytes_file_with_operations(
            &path,
            b"new",
            None,
            |_| Ok(()),
            AtomicWriteOperations {
                write_staged: write_staged_file,
                sync_staged: flush_and_sync_staged_file,
                before_replace: before_replace_noop,
                replace: replace_existing_file,
                sync_parent: fail_parent_sync,
            },
        )
        .expect_err("parent sync failure should be reported");

        assert!(matches!(err, AtomicWriteError::CommitStateUnknown { .. }));
        assert_eq!(fs::read(&path).expect("read committed file"), b"new");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn concurrent_writers_only_expose_complete_payloads() {
        const WRITERS: usize = 4;
        const WRITES_PER_WRITER: usize = 32;

        let directory = TestDir::new();
        let path = Arc::new(directory.join("state.json"));
        write_bytes_file(&path, br#"{"writer":0,"sequence":0,"padding":"seed"}"#)
            .expect("seed destination");
        let barrier = Arc::new(Barrier::new(WRITERS + 1));
        let mut writers = Vec::new();
        for writer in 0..WRITERS {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            writers.push(thread::spawn(move || {
                barrier.wait();
                for sequence in 0..WRITES_PER_WRITER {
                    let payload = serde_json::json!({
                        "writer": writer,
                        "sequence": sequence,
                        "padding": "x".repeat(16 * 1024),
                    });
                    write_bytes_file(&path, payload.to_string().as_bytes())
                        .expect("atomically replace concurrent payload");
                }
            }));
        }

        barrier.wait();
        while writers.iter().any(|writer| !writer.is_finished()) {
            let payload = fs::read(&*path).expect("read destination during replacement");
            serde_json::from_slice::<serde_json::Value>(&payload)
                .expect("destination should always contain one complete payload");
            thread::yield_now();
        }
        for writer in writers {
            writer.join().expect("writer thread should finish");
        }

        let payload = fs::read(&*path).expect("read final destination");
        serde_json::from_slice::<serde_json::Value>(&payload)
            .expect("final destination should contain a complete payload");
        assert!(managed_temp_files(&directory.0).is_empty());
    }

    #[test]
    fn stale_managed_temp_is_pruned_without_touching_unrelated_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        let stale_temp = directory.join(&format!(
            "{TEMP_FILE_PREFIX}{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let unrelated = directory.join(".codex-helper-tmp-v1-not-managed");
        let stale_time = SystemTime::now()
            .checked_sub(STALE_TEMP_FILE_AGE + Duration::from_secs(1))
            .expect("stale timestamp");
        for candidate in [&stale_temp, &unrelated] {
            let file = File::create(candidate).expect("create stale candidate");
            file.set_times(FileTimes::new().set_modified(stale_time))
                .expect("set stale modification time");
        }

        write_bytes_file(&path, b"new").expect("write destination");

        assert!(!stale_temp.exists(), "managed stale temp should be removed");
        assert!(unrelated.exists(), "unrelated file must remain untouched");
    }
}
