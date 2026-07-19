use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
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

#[derive(Clone, PartialEq, Eq)]
pub struct ManagedFileSnapshot {
    bytes: Option<Vec<u8>>,
}

impl ManagedFileSnapshot {
    pub fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }
}

impl std::fmt::Debug for ManagedFileSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedFileSnapshot")
            .field("exists", &self.bytes.is_some())
            .field("byte_len", &self.bytes.as_ref().map(Vec::len))
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum ManagedFileTransactionError {
    #[error("managed file transaction is already active for {path:?}")]
    Busy { path: PathBuf },
    #[error("managed file {path:?} changed concurrently")]
    ConcurrentChange { path: PathBuf },
    #[error("managed file operation {operation} failed for {path:?}: {source}")]
    Io {
        path: PathBuf,
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error(
        "managed file operation {operation} for {path:?} may have committed before an error was reported: {source}"
    )]
    CommitStateUnknown {
        path: PathBuf,
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug)]
struct ConcurrentManagedFileChange;

impl std::fmt::Display for ConcurrentManagedFileChange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("managed file changed concurrently")
    }
}

impl std::error::Error for ConcurrentManagedFileChange {}

pub struct ManagedFileTransaction {
    path: PathBuf,
    max_bytes: usize,
    _lock: File,
    original: ManagedFileSnapshot,
    current: ManagedFileSnapshot,
}

impl std::fmt::Debug for ManagedFileTransaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedFileTransaction")
            .field("path", &self.path)
            .field("original", &self.original)
            .field("current", &self.current)
            .finish_non_exhaustive()
    }
}

impl ManagedFileTransaction {
    pub fn begin(
        path: impl Into<PathBuf>,
        max_bytes: usize,
    ) -> Result<Self, ManagedFileTransactionError> {
        let path = path.into();
        let parent = destination_parent(&path)
            .map_err(|source| managed_file_io(&path, "resolve parent", source))?;
        fs::create_dir_all(&parent)
            .map_err(|source| managed_file_io(&path, "create parent", source))?;
        let lock_path = managed_file_lock_path(&path)
            .map_err(|source| managed_file_io(&path, "resolve transaction lock", source))?;
        let lock = open_managed_file_lock(&lock_path)
            .map_err(|source| managed_file_io(&path, "open transaction lock", source))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(ManagedFileTransactionError::Busy { path });
            }
            Err(TryLockError::Error(source)) => {
                return Err(managed_file_io(&path, "acquire transaction lock", source));
            }
        }
        validate_managed_regular_file(&lock_path)
            .map_err(|source| managed_file_io(&path, "validate transaction lock", source))?;
        let snapshot = read_managed_file_snapshot(&path, max_bytes)?;
        Ok(Self {
            path,
            max_bytes,
            _lock: lock,
            original: snapshot.clone(),
            current: snapshot,
        })
    }

    pub fn current(&self) -> &ManagedFileSnapshot {
        &self.current
    }

    pub fn replace(&mut self, data: &[u8]) -> Result<(), ManagedFileTransactionError> {
        if data.len() > self.max_bytes {
            return Err(managed_file_io(
                &self.path,
                "validate replacement size",
                managed_file_size_error(self.max_bytes),
            ));
        }
        self.ensure_current()?;
        if self.current.bytes() == Some(data) {
            return Ok(());
        }

        let expected = self.current.clone();
        let path = self.path.clone();
        let max_bytes = self.max_bytes;
        let result = write_bytes_file_validated_with_permissions_and_before_replace(
            &self.path,
            data,
            None,
            |_| Ok(()),
            move |_staged_path, destination| {
                let actual = read_managed_file_snapshot_io(destination, max_bytes)?;
                if actual != expected {
                    return Err(io::Error::other(ConcurrentManagedFileChange));
                }
                Ok(())
            },
        );
        match result {
            Ok(()) => {
                match read_managed_file_snapshot_io(&self.path, self.max_bytes) {
                    Ok(snapshot) => self.current = snapshot,
                    Err(source) => {
                        self.refresh_current_best_effort();
                        return Err(ManagedFileTransactionError::CommitStateUnknown {
                            path,
                            operation: "verify replace",
                            source,
                        });
                    }
                }
                if self.current.bytes() != Some(data) {
                    return Err(ManagedFileTransactionError::ConcurrentChange {
                        path: self.path.clone(),
                    });
                }
                Ok(())
            }
            Err(error) => {
                self.refresh_current_best_effort();
                Err(map_atomic_write_error(error, "replace"))
            }
        }
    }

    pub fn remove(&mut self) -> Result<(), ManagedFileTransactionError> {
        let parent = destination_parent(&self.path)
            .map_err(|source| managed_file_io(&self.path, "resolve parent", source))?;
        self.ensure_current()?;
        if self.current.bytes.is_none() {
            return Ok(());
        }
        match fs::remove_file(&self.path) {
            Ok(()) => {
                self.current = missing_managed_file_snapshot();
                sync_parent_directory(&parent).map_err(|source| {
                    ManagedFileTransactionError::CommitStateUnknown {
                        path: self.path.clone(),
                        operation: "remove",
                        source,
                    }
                })?;
                Ok(())
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                self.current = missing_managed_file_snapshot();
                Err(ManagedFileTransactionError::ConcurrentChange {
                    path: self.path.clone(),
                })
            }
            Err(source) => Err(managed_file_io(&self.path, "remove", source)),
        }
    }

    pub fn rollback(&mut self) -> Result<(), ManagedFileTransactionError> {
        self.ensure_current()?;
        if self.current == self.original {
            return Ok(());
        }
        match self.original.bytes.clone() {
            Some(bytes) => self.replace(&bytes),
            None => self.remove(),
        }
    }

    fn ensure_current(&self) -> Result<(), ManagedFileTransactionError> {
        let actual = read_managed_file_snapshot(&self.path, self.max_bytes)?;
        if actual == self.current {
            Ok(())
        } else {
            Err(ManagedFileTransactionError::ConcurrentChange {
                path: self.path.clone(),
            })
        }
    }

    fn refresh_current_best_effort(&mut self) {
        if let Ok(snapshot) = read_managed_file_snapshot(&self.path, self.max_bytes) {
            self.current = snapshot;
        }
    }
}

pub fn read_managed_file_snapshot(
    path: impl AsRef<Path>,
    max_bytes: usize,
) -> Result<ManagedFileSnapshot, ManagedFileTransactionError> {
    let path = path.as_ref();
    read_managed_file_snapshot_io(path, max_bytes)
        .map_err(|source| managed_file_io(path, "read snapshot", source))
}

fn missing_managed_file_snapshot() -> ManagedFileSnapshot {
    ManagedFileSnapshot { bytes: None }
}

fn read_managed_file_snapshot_io(path: &Path, max_bytes: usize) -> io::Result<ManagedFileSnapshot> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_managed_regular_file(path)?,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(missing_managed_file_snapshot());
        }
        Err(source) => return Err(source),
    }
    let file = File::open(path)?;
    if file.metadata()?.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(managed_file_size_error(max_bytes));
    }
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    file.take(
        u64::try_from(max_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )
    .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(managed_file_size_error(max_bytes));
    }
    validate_managed_regular_file(path)?;
    Ok(ManagedFileSnapshot { bytes: Some(bytes) })
}

fn managed_file_size_error(max_bytes: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("managed file exceeds the maximum size of {max_bytes} bytes"),
    )
}

fn validate_managed_regular_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed path must be a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if metadata.nlink() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed path must not have hard links",
            ));
        }
    }
    #[cfg(windows)]
    {
        let information = crate::windows_file_info::path_information_no_follow(path)?;
        if crate::windows_file_info::is_reparse_point(&information)
            || information.number_of_links() != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed path must not be a reparse point or hard link",
            ));
        }
    }
    Ok(())
}

fn managed_file_lock_path(path: &Path) -> io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "managed path must name a file")
    })?;
    let mut lock_name = file_name.to_os_string();
    lock_name.push(".lock");
    Ok(path.with_file_name(lock_name))
}

fn open_managed_file_lock(path: &Path) -> io::Result<File> {
    let mut create = OpenOptions::new();
    create.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        create.mode(0o600);
    }
    match create.open(path) {
        Ok(file) => Ok(file),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            validate_managed_regular_file(path)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .truncate(false)
                .open(path)?;
            validate_managed_regular_file(path)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "managed transaction lock is not a regular file",
                ));
            }
            Ok(file)
        }
        Err(source) => Err(source),
    }
}

fn managed_file_io(
    path: &Path,
    operation: &'static str,
    source: io::Error,
) -> ManagedFileTransactionError {
    ManagedFileTransactionError::Io {
        path: path.to_path_buf(),
        operation,
        source,
    }
}

fn map_atomic_write_error(
    error: AtomicWriteError,
    operation: &'static str,
) -> ManagedFileTransactionError {
    match error {
        AtomicWriteError::BeforeCommit { path, source, .. }
            if source
                .get_ref()
                .is_some_and(|error| error.is::<ConcurrentManagedFileChange>()) =>
        {
            ManagedFileTransactionError::ConcurrentChange { path }
        }
        AtomicWriteError::BeforeCommit { path, source, .. } => ManagedFileTransactionError::Io {
            path,
            operation,
            source,
        },
        AtomicWriteError::CommitStateUnknown { path, source, .. } => {
            ManagedFileTransactionError::CommitStateUnknown {
                path,
                operation,
                source,
            }
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
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        options
            .access_mode(DELETE | GENERIC_WRITE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_WRITE_THROUGH);
    }

    for _ in 0..TEMP_FILE_CREATE_ATTEMPTS {
        let path = parent.join(format!(
            "{TEMP_FILE_PREFIX}{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        match options.open(&path) {
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
fn replace_existing_file(
    staged_file: &File,
    staged_path: &Path,
    destination: &Path,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION,
        ERROR_INVALID_PARAMETER, ERROR_LOCK_VIOLATION, ERROR_NOT_SUPPORTED,
        ERROR_SHARING_VIOLATION,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_INFO_BY_HANDLE_CLASS, FILE_RENAME_INFO, FILE_RENAME_INFO_0, FileRenameInfo,
        FileRenameInfoEx, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        SetFileInformationByHandle,
    };

    const FILE_RENAME_REPLACE_IF_EXISTS: u32 = 0x1;
    const FILE_RENAME_POSIX_SEMANTICS: u32 = 0x2;

    fn encode_wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if path[..path.len().saturating_sub(1)].contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains an embedded null",
            ));
        }
        Ok(path)
    }

    fn canonical_destination_path(path: &Path) -> io::Result<PathBuf> {
        let file_name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination must include a file name",
            )
        })?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        Ok(fs::canonicalize(parent)?.join(file_name))
    }

    fn rename_buffer(path: &Path, anonymous: FILE_RENAME_INFO_0) -> io::Result<(Vec<usize>, u32)> {
        let path = encode_wide_path(path)?;
        let file_name_length = path
            .len()
            .saturating_sub(1)
            .checked_mul(size_of::<u16>())
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is too long"))?;
        let buffer_size = std::mem::offset_of!(FILE_RENAME_INFO, FileName)
            .checked_add(file_name_length as usize)
            .and_then(|length| length.checked_add(size_of::<u16>()))
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is too long"))?;
        let mut buffer = vec![0usize; (buffer_size as usize).div_ceil(size_of::<usize>())];
        let rename_info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();

        // SAFETY: The usize buffer is suitably aligned, is large enough for the variable-length
        // structure and null-terminated path, and remains alive while the API consumes it.
        unsafe {
            (*rename_info).Anonymous = anonymous;
            (*rename_info).RootDirectory = std::ptr::null_mut();
            (*rename_info).FileNameLength = file_name_length;
            path.as_ptr().copy_to_nonoverlapping(
                std::ptr::addr_of_mut!((*rename_info).FileName).cast::<u16>(),
                path.len(),
            );
        }
        Ok((buffer, buffer_size))
    }

    fn replace_with_file_info(
        staged_file: &File,
        rename_buffer: &mut [usize],
        rename_buffer_size: u32,
        information_class: FILE_INFO_BY_HANDLE_CLASS,
    ) -> io::Result<()> {
        let rename_info = rename_buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        let mut backoff = Duration::from_millis(1);
        for attempt in 0..WINDOWS_REPLACE_ATTEMPTS {
            // SAFETY: The handle and aligned rename buffer remain valid for the call.
            let replaced = unsafe {
                SetFileInformationByHandle(
                    staged_file.as_raw_handle(),
                    information_class,
                    rename_info.cast(),
                    rename_buffer_size,
                )
            };
            if replaced != 0 {
                return Ok(());
            }

            let error = io::Error::last_os_error();
            if !retryable_rename_error(&error) || attempt + 1 == WINDOWS_REPLACE_ATTEMPTS {
                return Err(error);
            }
            std::thread::sleep(backoff);
            backoff = std::cmp::min(backoff.saturating_mul(2), WINDOWS_REPLACE_MAX_BACKOFF);
        }
        unreachable!("the bounded replacement loop always returns on its final attempt")
    }

    fn retryable_rename_error(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_ACCESS_DENIED as i32
                    || code == ERROR_SHARING_VIOLATION as i32
                    || code == ERROR_LOCK_VIOLATION as i32
        )
    }

    fn extended_rename_is_unsupported(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_INVALID_FUNCTION as i32
                    || code == ERROR_NOT_SUPPORTED as i32
                    || code == ERROR_INVALID_PARAMETER as i32
                    || code == ERROR_CALL_NOT_IMPLEMENTED as i32
        )
    }

    fn replace_with_move_file(staged_path: &Path, destination: &Path) -> io::Result<()> {
        let staged_file_name = staged_path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "staging path must include a file name",
            )
        })?;
        let destination_parent = destination.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination must include a parent",
            )
        })?;
        let staged_path = encode_wide_path(&destination_parent.join(staged_file_name))?;
        let destination = encode_wide_path(destination)?;
        let mut backoff = Duration::from_millis(1);
        for attempt in 0..WINDOWS_REPLACE_ATTEMPTS {
            // SAFETY: Both buffers are null-terminated and remain alive for the call.
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

            let error = io::Error::last_os_error();
            if !retryable_rename_error(&error) || attempt + 1 == WINDOWS_REPLACE_ATTEMPTS {
                return Err(error);
            }
            std::thread::sleep(backoff);
            backoff = std::cmp::min(backoff.saturating_mul(2), WINDOWS_REPLACE_MAX_BACKOFF);
        }
        unreachable!("the bounded replacement loop always returns on its final attempt")
    }

    let destination = canonical_destination_path(destination)?;
    let (mut extended_buffer, extended_buffer_size) = rename_buffer(
        &destination,
        FILE_RENAME_INFO_0 {
            Flags: FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS,
        },
    )?;
    match replace_with_file_info(
        staged_file,
        &mut extended_buffer,
        extended_buffer_size,
        FileRenameInfoEx,
    ) {
        Ok(()) => return staged_file.sync_all(),
        Err(error) if extended_rename_is_unsupported(&error) => {}
        Err(error) => return Err(error),
    }

    let (mut standard_buffer, standard_buffer_size) = rename_buffer(
        &destination,
        FILE_RENAME_INFO_0 {
            ReplaceIfExists: true,
        },
    )?;
    match replace_with_file_info(
        staged_file,
        &mut standard_buffer,
        standard_buffer_size,
        FileRenameInfo,
    ) {
        Ok(()) => return staged_file.sync_all(),
        Err(error) if extended_rename_is_unsupported(&error) => {}
        Err(error) => return Err(error),
    }

    replace_with_move_file(staged_path, &destination)?;
    staged_file.sync_all()
}

#[cfg(not(windows))]
fn replace_existing_file(
    _staged_file: &File,
    staged_path: &Path,
    destination: &Path,
) -> io::Result<()> {
    fs::rename(staged_path, destination)
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    // The rename handle uses write-through I/O and is synced after the directory entry changes.
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
    R: FnOnce(&File, &Path, &Path) -> io::Result<()>,
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

    if let Err(err) = replace(&staged_file, staged_guard.path(), path) {
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
    write_bytes_file_validated_with_permissions_and_before_replace(
        path,
        data,
        permissions,
        validate,
        before_replace_noop,
    )
}

fn write_bytes_file_validated_with_permissions_and_before_replace<V, B>(
    path: &Path,
    data: &[u8],
    permissions: Option<fs::Permissions>,
    validate: V,
    before_replace: B,
) -> std::result::Result<(), AtomicWriteError>
where
    V: FnOnce(&[u8]) -> io::Result<()>,
    B: FnOnce(&Path, &Path) -> io::Result<()>,
{
    write_bytes_file_with_operations(
        path,
        data,
        permissions,
        validate,
        AtomicWriteOperations {
            write_staged: write_staged_file,
            sync_staged: flush_and_sync_staged_file,
            before_replace,
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

pub(crate) async fn write_bytes_file_async_with_permissions_and_before_replace<B>(
    path: &Path,
    data: &[u8],
    permissions: fs::Permissions,
    before_replace: B,
) -> std::result::Result<(), AtomicWriteError>
where
    B: FnOnce(&Path, &Path) -> io::Result<()> + Send + 'static,
{
    let path = path.to_path_buf();
    let error_path = path.clone();
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || {
        write_bytes_file_validated_with_permissions_and_before_replace(
            &path,
            &data,
            Some(permissions),
            |_| Ok(()),
            before_replace,
        )
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

    fn replace_then_report_error(
        staged_file: &File,
        staged_path: &Path,
        destination: &Path,
    ) -> io::Result<()> {
        replace_existing_file(staged_file, staged_path, destination)?;
        Err(io::Error::other("injected post-replace uncertainty"))
    }

    fn fail_replace_without_commit(
        _staged_file: &File,
        _staged_path: &Path,
        _destination: &Path,
    ) -> io::Result<()> {
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

    #[test]
    fn replacement_keeps_existing_reader_on_old_file_and_new_reads_on_new_file() {
        let directory = TestDir::new();
        let path = directory.join("state.json");
        fs::write(&path, "old").expect("write old file");
        let mut old_reader = File::open(&path).expect("open old destination");

        write_text_file(&path, "new").expect("replace destination while old reader remains open");

        let mut old_contents = String::new();
        old_reader
            .read_to_string(&mut old_contents)
            .expect("read old destination handle");
        assert_eq!(old_contents, "old");
        assert_eq!(
            fs::read_to_string(&path).expect("read replacement by path"),
            "new"
        );
    }

    #[test]
    fn managed_file_transaction_bounds_replacements_and_snapshot_reads() {
        let directory = TestDir::new();
        let path = directory.join("receipt.json");
        let mut transaction =
            ManagedFileTransaction::begin(&path, 3).expect("begin bounded transaction");

        assert!(matches!(
            transaction.replace(b"four"),
            Err(ManagedFileTransactionError::Io { .. })
        ));
        assert!(!path.exists(), "oversized replacement must not be written");

        transaction.replace(b"new").expect("write bounded payload");
        assert_eq!(fs::read(&path).expect("read bounded payload"), b"new");
        drop(transaction);

        fs::write(&path, b"four").expect("write oversized external payload");
        assert!(matches!(
            read_managed_file_snapshot(&path, 3),
            Err(ManagedFileTransactionError::Io { .. })
        ));
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
