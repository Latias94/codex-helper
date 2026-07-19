use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use zeroize::Zeroizing;

use super::model::{CredentialError, CredentialErrorCode, CredentialSourceKind, SecretValue};

const MAX_SECRET_FILE_BYTES: usize = 64 * 1024;

pub fn read_secret_file(path: impl AsRef<Path>) -> Result<SecretValue, CredentialError> {
    let path = path.as_ref();
    let reference = path.to_string_lossy().into_owned();
    if !path.is_absolute() {
        return Err(error(CredentialErrorCode::Invalid, reference));
    }

    let mut file = open_read_only(path).map_err(|source| io_error(source, &reference))?;
    let before = file
        .metadata()
        .map_err(|source| io_error(source, &reference))?;
    validate_regular_file(&before, &reference)?;

    let mut bytes = Zeroizing::new(Vec::with_capacity(
        usize::try_from(before.len())
            .unwrap_or(MAX_SECRET_FILE_BYTES)
            .min(MAX_SECRET_FILE_BYTES),
    ));
    (&mut file)
        .take((MAX_SECRET_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| io_error(source, &reference))?;

    let after = file
        .metadata()
        .map_err(|source| io_error(source, &reference))?;
    validate_regular_file(&after, &reference)?;
    let observed_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if before.len() != after.len()
        || after.len() != observed_len
        || bytes.len() > MAX_SECRET_FILE_BYTES
    {
        return Err(error(CredentialErrorCode::Invalid, reference));
    }

    if bytes.ends_with(b"\r\n") {
        let value_len = bytes.len() - 2;
        bytes.truncate(value_len);
    } else if bytes.ends_with(b"\n") {
        let value_len = bytes.len() - 1;
        bytes.truncate(value_len);
    }

    let value = std::mem::take(&mut *bytes);
    SecretValue::new(value).map_err(|_| error(CredentialErrorCode::Invalid, reference))
}

fn open_read_only(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    options.open(path)
}

fn validate_regular_file(
    metadata: &std::fs::Metadata,
    reference: &str,
) -> Result<(), CredentialError> {
    if !metadata.is_file()
        || metadata.len() > u64::try_from(MAX_SECRET_FILE_BYTES).unwrap_or(u64::MAX)
    {
        return Err(error(CredentialErrorCode::Invalid, reference));
    }
    Ok(())
}

fn io_error(source: std::io::Error, reference: &str) -> CredentialError {
    #[cfg(unix)]
    let invalid_special_file = matches!(
        source.raw_os_error(),
        Some(libc::ENXIO) | Some(libc::ENODEV)
    );
    #[cfg(not(unix))]
    let invalid_special_file = false;
    let code = if invalid_special_file {
        CredentialErrorCode::Invalid
    } else {
        match source.kind() {
            std::io::ErrorKind::NotFound => CredentialErrorCode::Missing,
            std::io::ErrorKind::PermissionDenied => CredentialErrorCode::PermissionDenied,
            std::io::ErrorKind::IsADirectory
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::Unsupported => CredentialErrorCode::Invalid,
            _ => CredentialErrorCode::BackendUnavailable,
        }
    };
    error(code, reference)
}

fn error(code: CredentialErrorCode, reference: impl Into<String>) -> CredentialError {
    CredentialError::new(code, CredentialSourceKind::SecretFile, reference)
}
