use std::io;
use std::path::{Path, PathBuf};

/// Resolves a path identity without requiring its final components to exist.
///
/// The nearest existing ancestor is canonicalized and missing components are
/// appended unchanged. Callers own policy such as requiring an absolute path.
pub fn resolve_path_identity(path: &Path) -> io::Result<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "no existing path ancestor is available",
                    )
                })?;
                missing.push(name.to_os_string());
                existing = existing.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "no existing path ancestor is available",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }

    let mut resolved = std::fs::canonicalize(existing)?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

pub fn path_identities_equal(left: &Path, right: &Path) -> bool {
    path_identities_equal_with_windows_semantics(left, right, cfg!(windows))
}

pub fn path_identities_equal_with_windows_semantics(
    left: &Path,
    right: &Path,
    windows_semantics: bool,
) -> bool {
    if windows_semantics {
        return left
            .to_str()
            .zip(right.to_str())
            .is_some_and(|(left, right)| windows_path_strings_equal(left, right));
    }
    left == right
}

pub fn windows_path_strings_equal(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        let normalized = value.replace('/', "\\").to_ascii_lowercase();
        let mut normalized = if let Some(path) = normalized.strip_prefix(r"\\?\unc\") {
            format!(r"\\{path}")
        } else if let Some(path) = normalized.strip_prefix(r"\\?\") {
            path.to_string()
        } else {
            normalized
        };
        while normalized.ends_with('\\') && !windows_path_is_root(&normalized) {
            normalized.pop();
        }
        normalized
    };
    normalize(left) == normalize(right)
}

fn windows_path_is_root(path: &str) -> bool {
    path == r"\"
        || path == r"\\"
        || matches!(path.as_bytes(), [drive, b':', b'\\'] if drive.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::path_identities_equal_with_windows_semantics;

    #[test]
    fn windows_identity_comparison_normalizes_aliases() {
        assert!(path_identities_equal_with_windows_semantics(
            Path::new(r"\\?\C:\Users\Operator\.Codex\missing"),
            Path::new("c:/users/operator/.codex/MISSING"),
            true,
        ));
        assert!(path_identities_equal_with_windows_semantics(
            Path::new(r"\\?\UNC\Server\Share\Codex"),
            Path::new(r"\\server\share\codex"),
            true,
        ));
        assert!(!path_identities_equal_with_windows_semantics(
            Path::new(r"C:\Users\Operator\.codex"),
            Path::new(r"C:\Users\Operator\.claude"),
            true,
        ));
    }
}
