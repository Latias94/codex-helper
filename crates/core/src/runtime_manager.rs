use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::proxy_home_dir;
use crate::logging::now_ms;
use crate::proxy::admin_port_for_proxy_port;

const RUNTIME_OWNER_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyLifecycleMode {
    EphemeralConsole,
    AttachedObserver,
    ResidentDaemon,
    DesktopOwned,
}

impl ProxyLifecycleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EphemeralConsole => "ephemeral_console",
            Self::AttachedObserver => "attached_observer",
            Self::ResidentDaemon => "resident_daemon",
            Self::DesktopOwned => "desktop_owned",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match normalize_token(value).as_str() {
            "ephemeral_console" | "ephemeral" | "console" | "owner" => Some(Self::EphemeralConsole),
            "attached_observer" | "attached" | "observer" | "attach" => {
                Some(Self::AttachedObserver)
            }
            "resident_daemon" | "resident" | "daemon" | "supervisor" => Some(Self::ResidentDaemon),
            "desktop_owned" | "desktop" | "tray" | "tauri" => Some(Self::DesktopOwned),
            _ => None,
        }
    }

    pub fn owns_runtime(self) -> bool {
        matches!(
            self,
            Self::EphemeralConsole | Self::ResidentDaemon | Self::DesktopOwned
        )
    }

    pub fn detach_on_normal_exit(self) -> bool {
        matches!(self, Self::AttachedObserver)
    }
}

impl fmt::Display for ProxyLifecycleMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ProxyLifecycleMode {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("unknown proxy lifecycle mode: {value}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOwnerKind {
    ManualCli,
    Supervisor,
    SystemService,
    Desktop,
}

impl RuntimeOwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualCli => "manual_cli",
            Self::Supervisor => "supervisor",
            Self::SystemService => "system_service",
            Self::Desktop => "desktop",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match normalize_token(value).as_str() {
            "manual_cli" | "manual" | "cli" | "resident" => Some(Self::ManualCli),
            "supervisor" | "watchdog" => Some(Self::Supervisor),
            "system_service" | "service" | "system" => Some(Self::SystemService),
            "desktop" | "desktop_owned" | "tray" | "tauri" => Some(Self::Desktop),
            _ => None,
        }
    }

    pub fn lifecycle_mode(self) -> ProxyLifecycleMode {
        match self {
            Self::ManualCli | Self::Supervisor | Self::SystemService => {
                ProxyLifecycleMode::ResidentDaemon
            }
            Self::Desktop => ProxyLifecycleMode::DesktopOwned,
        }
    }
}

impl fmt::Display for RuntimeOwnerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RuntimeOwnerKind {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| format!("unknown runtime owner kind: {value}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeOwnerMarker {
    pub schema_version: u32,
    #[serde(default)]
    pub instance_id: String,
    pub owner: RuntimeOwnerKind,
    pub lifecycle_mode: ProxyLifecycleMode,
    pub service_name: String,
    pub proxy_port: u16,
    pub admin_port: u16,
    pub pid: u32,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl RuntimeOwnerMarker {
    pub fn new(owner: RuntimeOwnerKind, service_name: impl Into<String>, proxy_port: u16) -> Self {
        Self::new_with_pid(
            owner,
            service_name,
            proxy_port,
            std::process::id(),
            now_ms(),
        )
    }

    pub fn new_with_pid(
        owner: RuntimeOwnerKind,
        service_name: impl Into<String>,
        proxy_port: u16,
        pid: u32,
        started_at_ms: u64,
    ) -> Self {
        Self {
            schema_version: RUNTIME_OWNER_SCHEMA_VERSION,
            instance_id: uuid::Uuid::new_v4().to_string(),
            owner,
            lifecycle_mode: owner.lifecycle_mode(),
            service_name: service_name.into(),
            proxy_port,
            admin_port: admin_port_for_proxy_port(proxy_port),
            pid,
            started_at_ms,
            supervisor_pid: None,
            note: None,
        }
    }

    pub fn with_supervisor_pid(mut self, supervisor_pid: u32) -> Self {
        self.supervisor_pid = Some(supervisor_pid);
        self
    }

    pub fn with_lifecycle_mode(mut self, lifecycle_mode: ProxyLifecycleMode) -> Self {
        self.lifecycle_mode = lifecycle_mode;
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        let note = note.into();
        if !note.trim().is_empty() {
            self.note = Some(note);
        }
        self
    }
}

pub fn runtime_run_dir() -> PathBuf {
    proxy_home_dir().join("run")
}

pub fn owner_marker_path(service_name: &str, proxy_port: u16) -> PathBuf {
    owner_marker_path_in(runtime_run_dir(), service_name, proxy_port)
}

pub fn owner_marker_path_in(
    run_dir: impl AsRef<Path>,
    service_name: &str,
    proxy_port: u16,
) -> PathBuf {
    run_dir.as_ref().join(format!(
        "{}-{}.owner.json",
        normalize_service_name(service_name),
        proxy_port
    ))
}

fn owner_lock_path_in(run_dir: impl AsRef<Path>, service_name: &str, proxy_port: u16) -> PathBuf {
    owner_marker_path_in(run_dir, service_name, proxy_port).with_extension("lock")
}

pub fn write_owner_marker(marker: &RuntimeOwnerMarker) -> Result<PathBuf> {
    write_owner_marker_to(runtime_run_dir(), marker)
}

pub fn write_owner_marker_to(
    run_dir: impl AsRef<Path>,
    marker: &RuntimeOwnerMarker,
) -> Result<PathBuf> {
    let run_dir = run_dir.as_ref();
    fs::create_dir_all(run_dir)
        .with_context(|| format!("create runtime run dir {}", run_dir.display()))?;
    let path = owner_marker_path_in(run_dir, &marker.service_name, marker.proxy_port);
    let text = serde_json::to_string_pretty(marker)?;
    fs::write(&path, text).with_context(|| format!("write owner marker {}", path.display()))?;
    Ok(path)
}

pub fn read_owner_marker(
    service_name: &str,
    proxy_port: u16,
) -> Result<Option<RuntimeOwnerMarker>> {
    read_owner_marker_from(runtime_run_dir(), service_name, proxy_port)
}

pub fn read_owner_marker_best_effort(
    service_name: &str,
    proxy_port: u16,
) -> Option<RuntimeOwnerMarker> {
    read_owner_marker_best_effort_from(runtime_run_dir(), service_name, proxy_port)
}

pub fn read_owner_marker_from(
    run_dir: impl AsRef<Path>,
    service_name: &str,
    proxy_port: u16,
) -> Result<Option<RuntimeOwnerMarker>> {
    let path = owner_marker_path_in(run_dir, service_name, proxy_port);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read owner marker {}", path.display()))?;
    let marker = serde_json::from_str::<RuntimeOwnerMarker>(&text)
        .with_context(|| format!("parse owner marker {}", path.display()))?;
    Ok(Some(marker))
}

pub fn read_owner_marker_best_effort_from(
    run_dir: impl AsRef<Path>,
    service_name: &str,
    proxy_port: u16,
) -> Option<RuntimeOwnerMarker> {
    match read_owner_marker_from(run_dir, service_name, proxy_port) {
        Ok(marker) => marker,
        Err(err) => {
            tracing::warn!(
                "ignoring unreadable runtime owner marker for {}:{}: {err}",
                service_name,
                proxy_port
            );
            None
        }
    }
}

pub fn clear_owner_marker(service_name: &str, proxy_port: u16) -> Result<()> {
    clear_owner_marker_from(runtime_run_dir(), service_name, proxy_port)
}

pub fn clear_owner_marker_from(
    run_dir: impl AsRef<Path>,
    service_name: &str,
    proxy_port: u16,
) -> Result<()> {
    let path = owner_marker_path_in(run_dir, service_name, proxy_port);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("clear owner marker {}", path.display())),
    }
}

#[derive(Debug)]
pub struct RuntimeOwnerLease {
    run_dir: PathBuf,
    marker: RuntimeOwnerMarker,
    _lock: File,
}

impl RuntimeOwnerLease {
    pub fn acquire(marker: &RuntimeOwnerMarker) -> Result<Self> {
        Self::acquire_in(runtime_run_dir(), marker)
    }

    pub fn acquire_in(run_dir: impl Into<PathBuf>, marker: &RuntimeOwnerMarker) -> Result<Self> {
        anyhow::ensure!(
            marker.schema_version == RUNTIME_OWNER_SCHEMA_VERSION,
            "runtime owner lease requires schema version {RUNTIME_OWNER_SCHEMA_VERSION}"
        );
        uuid::Uuid::parse_str(&marker.instance_id)
            .context("runtime owner lease requires a valid instance ID")?;
        anyhow::ensure!(
            marker.lifecycle_mode.owns_runtime(),
            "runtime owner lease requires an owning lifecycle mode"
        );
        anyhow::ensure!(
            marker.admin_port == admin_port_for_proxy_port(marker.proxy_port),
            "runtime owner marker admin port does not match its proxy port"
        );
        let run_dir = run_dir.into();
        fs::create_dir_all(&run_dir)
            .with_context(|| format!("create runtime run dir {}", run_dir.display()))?;
        let lock_path = owner_lock_path_in(&run_dir, &marker.service_name, marker.proxy_port);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(&lock_path)
            .with_context(|| format!("open runtime owner lock {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => anyhow::bail!(
                "runtime {}:{} is already owned; lock is held at {}",
                marker.service_name,
                marker.proxy_port,
                lock_path.display()
            ),
            Err(TryLockError::Error(error)) => {
                return Err(error)
                    .with_context(|| format!("lock runtime owner file {}", lock_path.display()));
            }
        }
        write_owner_marker_to(&run_dir, marker)?;
        Ok(Self {
            run_dir,
            marker: marker.clone(),
            _lock: lock,
        })
    }

    pub fn instance_id(&self) -> &str {
        self.marker.instance_id.as_str()
    }

    pub fn service_name(&self) -> &str {
        self.marker.service_name.as_str()
    }

    pub fn proxy_port(&self) -> u16 {
        self.marker.proxy_port
    }

    pub fn lifecycle_mode(&self) -> ProxyLifecycleMode {
        self.marker.lifecycle_mode
    }
}

impl Drop for RuntimeOwnerLease {
    fn drop(&mut self) {
        let current = read_owner_marker_from(
            &self.run_dir,
            &self.marker.service_name,
            self.marker.proxy_port,
        );
        match current {
            Ok(Some(current)) if current.instance_id == self.marker.instance_id => {
                if let Err(error) = clear_owner_marker_from(
                    &self.run_dir,
                    &self.marker.service_name,
                    self.marker.proxy_port,
                ) {
                    tracing::warn!("failed to clear owned runtime marker: {error}");
                }
            }
            Ok(Some(_)) | Ok(None) => {}
            Err(error) => {
                tracing::warn!("failed to verify runtime owner marker before release: {error}");
            }
        }
    }
}

#[derive(Debug)]
pub struct RuntimeOwnerMarkerGuard {
    run_dir: Option<PathBuf>,
    service_name: String,
    proxy_port: u16,
    enabled: bool,
}

impl RuntimeOwnerMarkerGuard {
    pub fn new(service_name: impl Into<String>, proxy_port: u16, enabled: bool) -> Self {
        Self {
            run_dir: None,
            service_name: service_name.into(),
            proxy_port,
            enabled,
        }
    }

    pub fn new_in(
        run_dir: impl Into<PathBuf>,
        service_name: impl Into<String>,
        proxy_port: u16,
        enabled: bool,
    ) -> Self {
        Self {
            run_dir: Some(run_dir.into()),
            service_name: service_name.into(),
            proxy_port,
            enabled,
        }
    }

    pub fn disarm(&mut self) {
        self.enabled = false;
    }
}

impl Drop for RuntimeOwnerMarkerGuard {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        let result = if let Some(run_dir) = self.run_dir.as_ref() {
            clear_owner_marker_from(run_dir, &self.service_name, self.proxy_port)
        } else {
            clear_owner_marker(&self.service_name, self.proxy_port)
        };
        if let Err(err) = result {
            tracing::warn!("failed to clear runtime owner marker: {err}");
        }
    }
}

pub fn describe_normal_exit(mode: ProxyLifecycleMode) -> &'static str {
    match mode {
        ProxyLifecycleMode::EphemeralConsole => "stop_owned_runtime",
        ProxyLifecycleMode::AttachedObserver => "detach_only",
        ProxyLifecycleMode::ResidentDaemon => "keep_resident_runtime",
        ProxyLifecycleMode::DesktopOwned => "keep_until_desktop_quit",
    }
}

fn normalize_token(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

fn normalize_service_name(value: &str) -> String {
    let normalized = normalize_token(value);
    if normalized.is_empty() {
        "codex".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_run_dir(test_name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "codex-helper-runtime-manager-{}-{}-{}",
            std::process::id(),
            test_name,
            now_ms()
        ));
        dir
    }

    #[test]
    fn lifecycle_modes_make_exit_policy_explicit() {
        assert_eq!(
            ProxyLifecycleMode::parse("ephemeral-console"),
            Some(ProxyLifecycleMode::EphemeralConsole)
        );
        assert_eq!(
            ProxyLifecycleMode::parse("attached"),
            Some(ProxyLifecycleMode::AttachedObserver)
        );
        assert_eq!(
            ProxyLifecycleMode::parse("desktop"),
            Some(ProxyLifecycleMode::DesktopOwned)
        );
        assert!(ProxyLifecycleMode::EphemeralConsole.owns_runtime());
        assert!(!ProxyLifecycleMode::AttachedObserver.owns_runtime());
        assert!(ProxyLifecycleMode::AttachedObserver.detach_on_normal_exit());
        assert_eq!(
            describe_normal_exit(ProxyLifecycleMode::DesktopOwned),
            "keep_until_desktop_quit"
        );
    }

    #[test]
    fn owner_kind_maps_to_lifecycle_mode() {
        assert_eq!(
            RuntimeOwnerKind::parse("manual-cli"),
            Some(RuntimeOwnerKind::ManualCli)
        );
        assert_eq!(
            RuntimeOwnerKind::parse("watchdog"),
            Some(RuntimeOwnerKind::Supervisor)
        );
        assert_eq!(
            RuntimeOwnerKind::parse("service"),
            Some(RuntimeOwnerKind::SystemService)
        );
        assert_eq!(
            RuntimeOwnerKind::parse("tray"),
            Some(RuntimeOwnerKind::Desktop)
        );
        assert_eq!(
            RuntimeOwnerKind::ManualCli.lifecycle_mode(),
            ProxyLifecycleMode::ResidentDaemon
        );
        assert_eq!(
            RuntimeOwnerKind::Desktop.lifecycle_mode(),
            ProxyLifecycleMode::DesktopOwned
        );
        assert_eq!(
            RuntimeOwnerKind::SystemService.lifecycle_mode(),
            ProxyLifecycleMode::ResidentDaemon
        );
    }

    #[test]
    fn owner_marker_round_trips_through_run_dir() {
        let run_dir = unique_run_dir("round-trip");
        let marker =
            RuntimeOwnerMarker::new_with_pid(RuntimeOwnerKind::Desktop, "codex", 3211, 42, 123456)
                .with_supervisor_pid(7)
                .with_note("started by desktop shell");

        let path = write_owner_marker_to(&run_dir, &marker).expect("write owner marker");
        assert_eq!(path, run_dir.join("codex-3211.owner.json"));

        let loaded = read_owner_marker_from(&run_dir, "codex", 3211)
            .expect("read owner marker")
            .expect("owner marker exists");
        assert_eq!(loaded.owner, RuntimeOwnerKind::Desktop);
        assert_eq!(loaded.lifecycle_mode, ProxyLifecycleMode::DesktopOwned);
        assert_eq!(loaded.admin_port, admin_port_for_proxy_port(3211));
        assert_eq!(loaded.pid, 42);
        assert!(!loaded.instance_id.is_empty());
        assert_eq!(loaded.supervisor_pid, Some(7));
        assert_eq!(loaded.note.as_deref(), Some("started by desktop shell"));

        clear_owner_marker_from(&run_dir, "codex", 3211).expect("clear owner marker");
        assert!(
            read_owner_marker_from(&run_dir, "codex", 3211)
                .expect("read after clear")
                .is_none()
        );
    }

    #[test]
    fn missing_owner_marker_is_not_an_error() {
        let run_dir = unique_run_dir("missing");
        assert!(
            read_owner_marker_from(&run_dir, "claude", 3210)
                .expect("missing marker read")
                .is_none()
        );
        clear_owner_marker_from(&run_dir, "claude", 3210).expect("missing marker clear");
    }

    #[test]
    fn corrupt_owner_marker_can_be_ignored_by_best_effort_reader() {
        let run_dir = unique_run_dir("corrupt");
        fs::create_dir_all(&run_dir).expect("create run dir");
        fs::write(owner_marker_path_in(&run_dir, "codex", 3211), "{not-json")
            .expect("write corrupt marker");

        assert!(read_owner_marker_from(&run_dir, "codex", 3211).is_err());
        assert!(read_owner_marker_best_effort_from(&run_dir, "codex", 3211).is_none());
    }

    #[test]
    fn owner_lease_rejects_a_second_live_owner_without_replacing_marker() {
        let run_dir = unique_run_dir("lease-exclusive");
        let marker =
            RuntimeOwnerMarker::new_with_pid(RuntimeOwnerKind::ManualCli, "codex", 3211, 42, 1);
        let lease = RuntimeOwnerLease::acquire_in(&run_dir, &marker).expect("acquire owner lease");
        let competing =
            RuntimeOwnerMarker::new_with_pid(RuntimeOwnerKind::Supervisor, "codex", 3211, 43, 2);

        let error = RuntimeOwnerLease::acquire_in(&run_dir, &competing)
            .expect_err("second live owner must be rejected");
        assert!(error.to_string().contains("already owned"), "{error}");
        assert_eq!(
            read_owner_marker_from(&run_dir, "codex", 3211)
                .expect("read marker")
                .expect("marker exists")
                .instance_id,
            marker.instance_id
        );

        drop(lease);
        assert!(
            read_owner_marker_from(&run_dir, "codex", 3211)
                .expect("read after lease drop")
                .is_none()
        );
    }

    #[test]
    fn owner_lease_rejects_legacy_or_non_owning_marker_identity() {
        let run_dir = unique_run_dir("lease-identity-validation");
        let mut marker =
            RuntimeOwnerMarker::new_with_pid(RuntimeOwnerKind::ManualCli, "codex", 3211, 42, 1);

        marker.schema_version = 1;
        assert!(
            RuntimeOwnerLease::acquire_in(&run_dir, &marker)
                .expect_err("legacy schema cannot own a runtime")
                .to_string()
                .contains("schema version")
        );

        marker.schema_version = RUNTIME_OWNER_SCHEMA_VERSION;
        marker.instance_id.clear();
        assert!(
            RuntimeOwnerLease::acquire_in(&run_dir, &marker)
                .expect_err("empty instance ID cannot own a runtime")
                .to_string()
                .contains("instance ID")
        );

        marker.instance_id = uuid::Uuid::new_v4().to_string();
        marker.lifecycle_mode = ProxyLifecycleMode::AttachedObserver;
        assert!(
            RuntimeOwnerLease::acquire_in(&run_dir, &marker)
                .expect_err("observer lifecycle cannot own a runtime")
                .to_string()
                .contains("owning lifecycle")
        );
    }

    #[test]
    fn dropping_old_owner_lease_preserves_a_replacement_marker() {
        let run_dir = unique_run_dir("lease-cas-drop");
        let original =
            RuntimeOwnerMarker::new_with_pid(RuntimeOwnerKind::ManualCli, "codex", 3211, 42, 1);
        let lease =
            RuntimeOwnerLease::acquire_in(&run_dir, &original).expect("acquire owner lease");
        let replacement =
            RuntimeOwnerMarker::new_with_pid(RuntimeOwnerKind::Desktop, "codex", 3211, 43, 2);
        write_owner_marker_to(&run_dir, &replacement).expect("replace marker out of band");

        drop(lease);

        assert_eq!(
            read_owner_marker_from(&run_dir, "codex", 3211)
                .expect("read replacement")
                .expect("replacement remains")
                .instance_id,
            replacement.instance_id
        );
    }
}
