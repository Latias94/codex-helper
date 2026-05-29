use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRetention {
    pub max_bytes: u64,
    pub max_files: usize,
}

impl LogRetention {
    pub const fn new(max_bytes: u64, max_files: usize) -> Self {
        Self {
            max_bytes,
            max_files,
        }
    }

    pub fn from_env(
        max_bytes_key: &str,
        max_files_key: &str,
        default_max_bytes: u64,
        default_max_files: usize,
    ) -> Self {
        Self {
            max_bytes: parse_u64_env(max_bytes_key).unwrap_or(default_max_bytes),
            max_files: parse_usize_env(max_files_key).unwrap_or(default_max_files),
        }
    }

    pub fn enabled(self) -> bool {
        self.max_bytes > 0 && self.max_files > 0
    }

    fn rotated_budget_bytes(self) -> u64 {
        self.max_bytes.saturating_mul(self.max_files as u64)
    }
}

#[derive(Debug)]
pub struct RotatedLogFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: SystemTime,
}

pub fn repair_log(path: impl AsRef<Path>, retention: LogRetention) {
    let path = path.as_ref();
    rotate_and_prune_if_needed(path, retention);
    prune_rotated_logs(path, retention);
}

pub fn append_line(path: impl AsRef<Path>, retention: LogRetention, line: &str) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    rotate_and_prune_if_needed(path, retention);
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn rotate_and_prune_if_needed(path: impl AsRef<Path>, retention: LogRetention) {
    let path = path.as_ref();
    if !retention.enabled() {
        return;
    }
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() < retention.max_bytes {
        prune_rotated_logs(path, retention);
        return;
    }
    if rotate_log_path(path).is_ok() {
        prune_rotated_logs(path, retention);
    }
}

pub fn prune_rotated_logs(path: impl AsRef<Path>, retention: LogRetention) {
    let path = path.as_ref();
    prune_rotated_logs_with_remover(path, retention, |candidate| fs::remove_file(candidate));
}

fn prune_rotated_logs_with_remover(
    path: &Path,
    retention: LogRetention,
    mut remove_file: impl FnMut(&Path) -> io::Result<()>,
) {
    if !retention.enabled() {
        return;
    }
    let mut rotated = collect_rotated_logs(path);
    let mut total_bytes = rotated
        .iter()
        .fold(0_u64, |acc, file| acc.saturating_add(file.bytes));
    let budget_bytes = retention.rotated_budget_bytes();

    while !rotated.is_empty() && (rotated.len() > retention.max_files || total_bytes > budget_bytes)
    {
        let file = rotated.remove(0);
        match remove_file(&file.path) {
            Ok(()) => {
                total_bytes = total_bytes.saturating_sub(file.bytes);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                total_bytes = total_bytes.saturating_sub(file.bytes);
            }
            Err(_) => {}
        }
    }
}

pub fn collect_rotated_logs(path: impl AsRef<Path>) -> Vec<RotatedLogFile> {
    let path = path.as_ref();
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Ok(rd) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut rotated: Vec<RotatedLogFile> = rd
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let entry_path = entry.path();
            if !is_rotated_log_path(path, &entry_path) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            Some(RotatedLogFile {
                path: entry_path,
                bytes: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect();
    rotated.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    rotated
}

pub struct RotatingLogWriter {
    path: PathBuf,
    retention: LogRetention,
    file: Option<File>,
    current_len: u64,
}

impl RotatingLogWriter {
    pub fn new(path: PathBuf, retention: LogRetention) -> Self {
        let current_len = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Self {
            path,
            retention,
            file: None,
            current_len,
        }
    }

    fn ensure_file(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.current_len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
            self.file = Some(file);
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("bounded log file was not opened"))
    }

    fn rotate_before_write(&mut self, incoming_len: usize) {
        if !self.retention.enabled() || self.current_len == 0 {
            return;
        }
        let incoming_len = incoming_len as u64;
        if self.current_len.saturating_add(incoming_len) < self.retention.max_bytes {
            return;
        }
        self.file.take();
        if rotate_log_path(&self.path).is_ok() {
            self.current_len = 0;
            prune_rotated_logs(&self.path, self.retention);
        } else {
            self.current_len = fs::metadata(&self.path).map(|meta| meta.len()).unwrap_or(0);
        }
    }
}

impl Write for RotatingLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.rotate_before_write(buf.len());
        let file = self.ensure_file()?;
        let written = file.write(buf)?;
        self.current_len = self.current_len.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()
        } else {
            Ok(())
        }
    }
}

fn rotate_log_path(path: &Path) -> io::Result<()> {
    for attempt in 0..100 {
        let rotated_path = rotated_path_for(path, attempt);
        if rotated_path.exists() {
            continue;
        }
        return fs::rename(path, rotated_path);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate rotated log file name",
    ))
}

fn rotated_path_for(path: &Path, attempt: u32) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let attempt_suffix = if attempt == 0 {
        String::new()
    } else {
        format!(".{attempt}")
    };
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("log");

    if path.extension().and_then(|s| s.to_str()) == Some("jsonl")
        && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
    {
        return path.with_file_name(format!("{stem}.{ts}{attempt_suffix}.jsonl"));
    }

    path.with_file_name(format!("{file_name}.{ts}{attempt_suffix}"))
}

fn is_rotated_log_path(active_path: &Path, candidate_path: &Path) -> bool {
    if active_path == candidate_path {
        return false;
    }
    let Some(name) = candidate_path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(active_name) = active_path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };

    if active_path.extension().and_then(|s| s.to_str()) == Some("jsonl")
        && let Some(stem) = active_path.file_stem().and_then(|s| s.to_str())
    {
        return name.starts_with(&format!("{stem}.")) && name.ends_with(".jsonl");
    }

    name.starts_with(&format!("{active_name}."))
}

fn parse_u64_env(key: &str) -> Option<u64> {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}

fn parse_usize_env(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "codex-helper-local-log-store-{test_name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp log dir");
        dir
    }

    fn write_test_file(path: &Path, bytes: usize) {
        let mut file = File::create(path).expect("create test log file");
        file.write_all(&vec![b'x'; bytes])
            .expect("write test log file");
        file.flush().expect("flush test log file");
    }

    #[test]
    fn repair_log_removes_legacy_runtime_rotated_file_over_budget() {
        let dir = temp_log_dir("legacy-runtime-rotated-budget");
        let active = dir.join("runtime.log");
        let legacy = dir.join("runtime.log.legacy");
        write_test_file(&legacy, 64);

        repair_log(&active, LogRetention::new(10, 2));

        assert!(
            !legacy.exists(),
            "oversized legacy rotated runtime log should be removed"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repair_log_rotates_and_prunes_oversized_active_file() {
        let dir = temp_log_dir("active-oversized");
        let active = dir.join("runtime.log");
        write_test_file(&active, 24);

        repair_log(&active, LogRetention::new(10, 2));

        assert!(
            !active.exists(),
            "oversized active log should be rotated away before tracing starts"
        );
        assert!(
            collect_rotated_logs(&active).is_empty(),
            "rotated oversized active log should be pruned by total budget"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_rotated_logs_keeps_budget_pressure_when_delete_fails() {
        let dir = temp_log_dir("delete-fail-budget-pressure");
        let active = dir.join("runtime.log");
        let locked = dir.join("runtime.log.0-locked");
        let spill = dir.join("runtime.log.1-spill");
        write_test_file(&locked, 64);
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_test_file(&spill, 8);

        prune_rotated_logs_with_remover(
            &active,
            LogRetention::new(10, 1),
            |path| -> io::Result<()> {
                if path == locked {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "locked"));
                }
                fs::remove_file(path)
            },
        );

        assert!(
            locked.exists(),
            "failed delete should leave the locked rotated log for a later repair"
        );
        assert!(
            !spill.exists(),
            "failed delete must not be counted as recovered budget"
        );

        prune_rotated_logs(&active, LogRetention::new(10, 1));
        assert!(
            !locked.exists(),
            "next repair should remove the previously locked oversized log"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rotating_log_writer_rotates_while_process_is_running() {
        let dir = temp_log_dir("writer-rotates");
        let active = dir.join("runtime.log");
        let retention = LogRetention::new(16, 2);
        let mut writer = RotatingLogWriter::new(active.clone(), retention);

        writer
            .write_all(b"first-line-000\n")
            .expect("write first line");
        writer
            .write_all(b"second-line-00\n")
            .expect("write second line");
        writer.flush().expect("flush runtime log writer");
        drop(writer);

        assert!(active.exists(), "new active log should exist");
        assert!(
            fs::metadata(&active).expect("active metadata").len() <= retention.max_bytes,
            "active log should stay under the configured size after rotation"
        );
        assert_eq!(
            collect_rotated_logs(&active).len(),
            1,
            "first active log should be rotated during the same process"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_line_rotates_jsonl_before_extension() {
        let dir = temp_log_dir("jsonl-rotation");
        let active = dir.join("control_trace.jsonl");
        write_test_file(&active, 24);

        append_line(&active, LogRetention::new(16, 2), "{\"ok\":true}").expect("append jsonl line");

        assert!(active.exists(), "append should recreate active jsonl file");
        let rotated = collect_rotated_logs(&active);
        assert_eq!(rotated.len(), 1);
        let rotated_name = rotated[0]
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("rotated file name");
        assert!(rotated_name.starts_with("control_trace."));
        assert!(rotated_name.ends_with(".jsonl"));
        let _ = fs::remove_dir_all(dir);
    }
}
