use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::time::Duration;

use serde::Serialize;

use crate::cli_types::{CliError, CliResult, ServiceCommand};
use crate::config::proxy_home_dir;

#[cfg(windows)]
const WINDOWS_SERVICE_NAME: &str = "codex-helper";
#[cfg(target_os = "macos")]
const MACOS_LABEL: &str = "io.github.latias94.codex-helper";
#[cfg(target_os = "linux")]
const LINUX_UNIT_NAME: &str = "codex-helper.service";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServicePlatform {
    Windows,
    Macos,
    Linux,
    Unsupported,
}

impl ServicePlatform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Unsupported
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub(crate) enum ServiceRuntimeState {
    Running,
    Stopped,
    Starting,
    Stopping,
    Installed,
    NotInstalled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ServiceStatus {
    platform: ServicePlatform,
    service_name: String,
    state: ServiceRuntimeState,
    installed: bool,
    autostart: bool,
    service_definition: Option<PathBuf>,
    log_directory: PathBuf,
    detail: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ServiceInstallOptions {
    pub(crate) service_name: &'static str,
    pub(crate) host: IpAddr,
    pub(crate) port: u16,
    pub(crate) start: bool,
    pub(crate) helper_home: PathBuf,
}

pub(crate) async fn handle_service_command(command: ServiceCommand) -> CliResult<()> {
    match command {
        ServiceCommand::Install {
            codex,
            claude,
            host,
            port,
            no_start,
        } => {
            let service_name = service_name_from_flags(codex, claude)?;
            let port = port.unwrap_or_else(|| default_proxy_port(service_name));
            install(ServiceInstallOptions {
                service_name,
                host,
                port,
                start: !no_start,
                helper_home: proxy_home_dir(),
            })?;
            print_status(&status()?, false)?;
        }
        ServiceCommand::Uninstall { keep_running } => uninstall(!keep_running)?,
        ServiceCommand::Start => start()?,
        ServiceCommand::Stop => stop()?,
        ServiceCommand::Restart => {
            stop()?;
            start()?;
        }
        ServiceCommand::Status { json } => print_status(&status()?, json)?,
        ServiceCommand::Logs => print_logs(),
        ServiceCommand::Run {
            service_name,
            host,
            port,
            helper_home,
        } => {
            let service_name = service_name_from_value(&service_name)?;
            let helper_home = helper_home.unwrap_or_else(proxy_home_dir);
            configure_service_process(&helper_home);
            run_service_dispatcher(ServiceInstallOptions {
                service_name,
                host,
                port: port.unwrap_or_else(|| default_proxy_port(service_name)),
                start: false,
                helper_home,
            })?;
        }
    }
    Ok(())
}

fn service_name_from_value(value: &str) -> CliResult<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "codex" => Ok("codex"),
        "claude" => Ok("claude"),
        _ => Err(CliError::Other(format!(
            "unsupported service name '{value}'; expected codex or claude"
        ))),
    }
}

fn configure_service_process(helper_home: &Path) {
    unsafe {
        std::env::set_var("CODEX_HELPER_HOME", helper_home);
    }
}

fn service_name_from_flags(codex: bool, claude: bool) -> CliResult<&'static str> {
    match (codex, claude) {
        (true, true) => Err(CliError::Other(
            "--codex and --claude are mutually exclusive".to_string(),
        )),
        (_, true) => Ok("claude"),
        _ => Ok("codex"),
    }
}

fn default_proxy_port(service_name: &str) -> u16 {
    if service_name == "claude" { 3210 } else { 3211 }
}

fn print_status(status: &ServiceStatus, json: bool) -> CliResult<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(status)
                .map_err(|error| CliError::Other(error.to_string()))?
        );
        return Ok(());
    }
    println!("codex-helper service");
    println!("  platform: {:?}", status.platform);
    println!("  installed: {}", status.installed);
    println!("  autostart: {}", status.autostart);
    println!("  state: {:?}", status.state);
    if let Some(path) = status.service_definition.as_ref() {
        println!("  definition: {}", path.display());
    }
    println!("  logs: {}", status.log_directory.display());
    if let Some(detail) = status.detail.as_deref() {
        println!("  detail: {detail}");
    }
    Ok(())
}

fn print_logs() {
    let log_dir = service_log_dir();
    println!("{}", log_dir.display());
    for file_name in ["service.stdout.log", "service.stderr.log"] {
        let path = log_dir.join(file_name);
        if path.exists() {
            println!("  {}", path.display());
        }
    }
}

fn service_log_dir() -> PathBuf {
    proxy_home_dir().join("logs")
}

fn current_executable() -> CliResult<PathBuf> {
    std::env::current_exe()
        .map_err(|error| CliError::Other(format!("resolve current executable: {error}")))
}

fn ensure_service_log_dir() -> CliResult<PathBuf> {
    let path = service_log_dir();
    std::fs::create_dir_all(&path).map_err(|error| {
        CliError::Other(format!(
            "create service log directory {}: {error}",
            path.display()
        ))
    })?;
    Ok(path)
}

fn run_command(program: &str, args: &[OsString]) -> CliResult<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| CliError::Other(format!("run {program}: {error}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(CliError::Other(format!(
        "{program} failed with {}: {detail}",
        output.status
    )))
}

#[cfg(windows)]
fn install(options: ServiceInstallOptions) -> CliResult<()> {
    windows::install(options)
}

#[cfg(target_os = "macos")]
fn install(options: ServiceInstallOptions) -> CliResult<()> {
    macos::install(options)
}

#[cfg(target_os = "linux")]
fn install(options: ServiceInstallOptions) -> CliResult<()> {
    linux::install(options)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn install(_options: ServiceInstallOptions) -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn uninstall(stop_first: bool) -> CliResult<()> {
    windows::uninstall(stop_first)
}

#[cfg(target_os = "macos")]
fn uninstall(stop_first: bool) -> CliResult<()> {
    macos::uninstall(stop_first)
}

#[cfg(target_os = "linux")]
fn uninstall(stop_first: bool) -> CliResult<()> {
    linux::uninstall(stop_first)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn uninstall(_stop_first: bool) -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn start() -> CliResult<()> {
    windows::start()
}

#[cfg(target_os = "macos")]
fn start() -> CliResult<()> {
    macos::start()
}

#[cfg(target_os = "linux")]
fn start() -> CliResult<()> {
    linux::start()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn start() -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn stop() -> CliResult<()> {
    windows::stop()
}

#[cfg(target_os = "macos")]
fn stop() -> CliResult<()> {
    macos::stop()
}

#[cfg(target_os = "linux")]
fn stop() -> CliResult<()> {
    linux::stop()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn stop() -> CliResult<()> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn status() -> CliResult<ServiceStatus> {
    windows::status()
}

#[cfg(target_os = "macos")]
fn status() -> CliResult<ServiceStatus> {
    macos::status()
}

#[cfg(target_os = "linux")]
fn status() -> CliResult<ServiceStatus> {
    linux::status()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn status() -> CliResult<ServiceStatus> {
    Err(unsupported_platform())
}

#[cfg(windows)]
fn run_service_dispatcher(options: ServiceInstallOptions) -> CliResult<()> {
    windows::run_dispatcher(options)
}

#[cfg(not(windows))]
fn run_service_dispatcher(_options: ServiceInstallOptions) -> CliResult<()> {
    Err(CliError::Other(
        "service run is an internal Windows Service Control Manager entrypoint".to_string(),
    ))
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn unsupported_platform() -> CliError {
    CliError::Other("system service management is unsupported on this platform".to_string())
}

#[cfg(windows)]
mod windows {
    use std::ffi::{OsStr, OsString};
    use std::sync::{OnceLock, mpsc};

    use windows_service::define_windows_service;
    use windows_service::service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
        ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus as WindowsStatus, ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::service_dispatcher;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    use super::*;

    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
    static SERVICE_OPTIONS: OnceLock<ServiceInstallOptions> = OnceLock::new();

    define_windows_service!(service_entry, service_main);

    pub(super) fn install(options: ServiceInstallOptions) -> CliResult<()> {
        ensure_service_log_dir()?;
        let executable = current_executable()?;
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .map_err(windows_error("open Windows Service Control Manager"))?;
        let service_info = ServiceInfo {
            name: OsString::from(WINDOWS_SERVICE_NAME),
            display_name: OsString::from("codex-helper relay"),
            service_type: SERVICE_TYPE,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: executable,
            launch_arguments: vec![
                OsString::from("service"),
                OsString::from("run"),
                OsString::from("--service-name"),
                OsString::from(options.service_name),
                OsString::from("--host"),
                OsString::from(options.host.to_string()),
                OsString::from("--port"),
                OsString::from(options.port.to_string()),
                OsString::from("--helper-home"),
                options.helper_home.clone().into_os_string(),
            ],
            dependencies: vec![],
            account_name: None,
            account_password: None,
        };
        let access = ServiceAccess::QUERY_STATUS
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::DELETE
            | ServiceAccess::CHANGE_CONFIG
            | ServiceAccess::QUERY_CONFIG;
        let service = manager
            .create_service(&service_info, access)
            .or_else(|_| manager.open_service(WINDOWS_SERVICE_NAME, access))
            .map_err(windows_error("create or open codex-helper Windows service"))?;
        service
            .change_config(&service_info)
            .map_err(windows_error("update codex-helper Windows service"))?;
        service
            .set_description(
                "Resident codex-helper relay. The tray and TUI attach to this service.",
            )
            .map_err(windows_error("set Windows service description"))?;
        service
            .update_failure_actions(ServiceFailureActions {
                reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(300)),
                reboot_msg: None,
                command: None,
                actions: Some(vec![
                    ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(2),
                    },
                    ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(10),
                    },
                    ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(30),
                    },
                ]),
            })
            .map_err(windows_error("set Windows service failure actions"))?;
        service
            .set_failure_actions_on_non_crash_failures(true)
            .map_err(windows_error("enable Windows service failure actions"))?;
        service
            .set_delayed_auto_start(true)
            .map_err(windows_error("set delayed Windows service startup"))?;
        if options.start {
            service
                .start::<&OsStr>(&[])
                .map_err(windows_error("start codex-helper Windows service"))?;
        }
        Ok(())
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = manager
            .open_service(
                WINDOWS_SERVICE_NAME,
                ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
            )
            .map_err(windows_error("open codex-helper Windows service"))?;
        if stop_first
            && service
                .query_status()
                .map_err(windows_error("query Windows service status"))?
                .current_state
                != ServiceState::Stopped
        {
            service
                .stop()
                .map_err(windows_error("stop codex-helper Windows service"))?;
        }
        service
            .delete()
            .map_err(windows_error("delete codex-helper Windows service"))
    }

    pub(super) fn start() -> CliResult<()> {
        with_service(
            ServiceAccess::START,
            |service| service.start::<&OsStr>(&[]),
            "start codex-helper Windows service",
        )
    }

    pub(super) fn stop() -> CliResult<()> {
        with_service(
            ServiceAccess::STOP,
            |service| service.stop().map(|_| ()),
            "stop codex-helper Windows service",
        )
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = match manager.open_service(
            WINDOWS_SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG,
        ) {
            Ok(service) => service,
            Err(_) => return Ok(base_status(ServiceRuntimeState::NotInstalled, false, false)),
        };
        let raw = service
            .query_status()
            .map_err(windows_error("query Windows service status"))?;
        let config = service
            .query_config()
            .map_err(windows_error("query Windows service config"))?;
        let state = match raw.current_state {
            ServiceState::Running => ServiceRuntimeState::Running,
            ServiceState::Stopped => ServiceRuntimeState::Stopped,
            ServiceState::StartPending => ServiceRuntimeState::Starting,
            ServiceState::StopPending => ServiceRuntimeState::Stopping,
            _ => ServiceRuntimeState::Unknown,
        };
        let mut status = base_status(
            state,
            true,
            config.start_type == ServiceStartType::AutoStart,
        );
        status.detail = raw.process_id.map(|pid| format!("pid={pid}"));
        Ok(status)
    }

    pub(super) fn run_dispatcher(options: ServiceInstallOptions) -> CliResult<()> {
        SERVICE_OPTIONS.set(options).map_err(|_| {
            CliError::Other("Windows service options were initialized twice".to_string())
        })?;
        service_dispatcher::start(WINDOWS_SERVICE_NAME, service_entry)
            .map_err(windows_error("start Windows service dispatcher"))
    }

    fn service_main(arguments: Vec<OsString>) {
        if let Err(error) = run_service(arguments) {
            tracing::error!(error = %error, "Windows service stopped with an error");
        }
    }

    fn run_service(_arguments: Vec<OsString>) -> CliResult<()> {
        let options = SERVICE_OPTIONS.get().cloned().ok_or_else(|| {
            CliError::Other("Windows service options are unavailable".to_string())
        })?;
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let event_handler = move |control| match control {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = shutdown_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status_handle = service_control_handler::register(WINDOWS_SERVICE_NAME, event_handler)
            .map_err(windows_error("register Windows service control handler"))?;
        report_status(&status_handle, ServiceState::StartPending, false)?;
        let executable = current_executable()?;
        let service_flag = if options.service_name == "claude" {
            "--claude"
        } else {
            "--codex"
        };
        let child_args = vec![
            OsString::from("serve"),
            OsString::from(service_flag),
            OsString::from("--host"),
            OsString::from(options.host.to_string()),
            OsString::from("--port"),
            OsString::from(options.port.to_string()),
            OsString::from("--no-tui"),
            OsString::from("--service-managed"),
        ];
        let mut child = Command::new(executable)
            .args(child_args)
            .spawn()
            .map_err(|error| CliError::Other(format!("start service relay child: {error}")))?;
        report_status(&status_handle, ServiceState::Running, true)?;
        loop {
            if shutdown_rx.recv_timeout(Duration::from_millis(250)).is_ok() {
                let _ = child.kill();
                let _ = child.wait();
                report_status(&status_handle, ServiceState::Stopped, false)?;
                return Ok(());
            }
            if let Some(status) = child.try_wait().map_err(|error| {
                CliError::Other(format!("wait for service relay child: {error}"))
            })? {
                report_status(&status_handle, ServiceState::Stopped, false)?;
                return Err(CliError::Other(format!(
                    "service relay child exited unexpectedly: {status}"
                )));
            }
        }
    }

    fn report_status(
        handle: &ServiceStatusHandle,
        current_state: ServiceState,
        accepts_stop: bool,
    ) -> CliResult<()> {
        handle
            .set_service_status(WindowsStatus {
                service_type: SERVICE_TYPE,
                current_state,
                controls_accepted: if accepts_stop {
                    ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
                } else {
                    ServiceControlAccept::empty()
                },
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(10),
                process_id: None,
            })
            .map_err(windows_error("report Windows service status"))
    }

    fn with_service<T>(
        access: ServiceAccess,
        operation: impl FnOnce(&windows_service::service::Service) -> windows_service::Result<T>,
        action: &'static str,
    ) -> CliResult<T> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(windows_error("open Windows Service Control Manager"))?;
        let service = manager
            .open_service(WINDOWS_SERVICE_NAME, access)
            .map_err(windows_error("open codex-helper Windows service"))?;
        operation(&service).map_err(windows_error(action))
    }

    fn windows_error(action: &'static str) -> impl FnOnce(windows_service::Error) -> CliError {
        move |error| CliError::Other(format!("{action}: {error}"))
    }

    fn base_status(state: ServiceRuntimeState, installed: bool, autostart: bool) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: WINDOWS_SERVICE_NAME.to_string(),
            state,
            installed,
            autostart,
            service_definition: None,
            log_directory: service_log_dir(),
            detail: None,
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub(super) fn install(options: ServiceInstallOptions) -> CliResult<()> {
        let executable = current_executable()?;
        let log_dir = ensure_service_log_dir()?;
        let path = launch_agent_path()?;
        let document = render_launch_agent(&executable, &log_dir, &options);
        write_service_definition(&path, document.as_bytes())?;
        let domain = launchd_domain()?;
        let _ = run_command(
            "launchctl",
            &[
                OsString::from("bootout"),
                domain.clone(),
                path.clone().into_os_string(),
            ],
        );
        run_command(
            "launchctl",
            &[OsString::from("bootstrap"), domain, path.into_os_string()],
        )?;
        if options.start {
            start()?;
        }
        Ok(())
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        let path = launch_agent_path()?;
        if stop_first && path.exists() {
            let _ = run_command(
                "launchctl",
                &[
                    OsString::from("bootout"),
                    launchd_domain()?,
                    path.clone().into_os_string(),
                ],
            );
        }
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(CliError::Other(format!(
                "remove launch agent {}: {error}",
                path.display()
            ))),
        }
    }

    pub(super) fn start() -> CliResult<()> {
        run_command(
            "launchctl",
            &[
                OsString::from("kickstart"),
                OsString::from("-k"),
                OsString::from(format!("{}/{MACOS_LABEL}", launchd_domain_string()?)),
            ],
        )
        .map(|_| ())
    }

    pub(super) fn stop() -> CliResult<()> {
        run_command(
            "launchctl",
            &[
                OsString::from("kill"),
                OsString::from("SIGTERM"),
                OsString::from(format!("{}/{MACOS_LABEL}", launchd_domain_string()?)),
            ],
        )
        .map(|_| ())
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let path = launch_agent_path()?;
        if !path.exists() {
            return Ok(base_status(
                ServiceRuntimeState::NotInstalled,
                false,
                false,
                Some(path),
            ));
        }
        let target = format!("{}/{MACOS_LABEL}", launchd_domain_string()?);
        let output = run_command(
            "launchctl",
            &[OsString::from("print"), OsString::from(target)],
        );
        let (state, detail) = match output {
            Ok(output) if output.contains("state = running") => {
                (ServiceRuntimeState::Running, Some(output))
            }
            Ok(output) => (ServiceRuntimeState::Stopped, Some(output)),
            Err(error) => (ServiceRuntimeState::Installed, Some(error.to_string())),
        };
        let mut status = base_status(state, true, true, Some(path));
        status.detail = detail;
        Ok(status)
    }

    fn launch_agent_path() -> CliResult<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| CliError::Other("resolve home directory".to_string()))?;
        Ok(home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{MACOS_LABEL}.plist")))
    }

    fn launchd_domain_string() -> CliResult<String> {
        let output = run_command("id", &[OsString::from("-u")])?;
        if output.is_empty() || !output.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(CliError::Other("resolve launchd GUI user id".to_string()));
        }
        Ok(format!("gui/{output}"))
    }

    fn launchd_domain() -> CliResult<OsString> {
        launchd_domain_string().map(OsString::from)
    }

    pub(super) fn render_launch_agent(
        executable: &Path,
        log_dir: &Path,
        options: &ServiceInstallOptions,
    ) -> String {
        let service_flag = if options.service_name == "claude" {
            "--claude"
        } else {
            "--codex"
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{MACOS_LABEL}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>serve</string><string>{service_flag}</string><string>--host</string><string>{}</string><string>--port</string><string>{}</string><string>--no-tui</string><string>--service-managed</string></array>\n<key>EnvironmentVariables</key><dict><key>CODEX_HELPER_HOME</key><string>{}</string></dict>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n<key>ThrottleInterval</key><integer>10</integer>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            xml_escape(executable.to_string_lossy().as_ref()),
            options.host,
            options.port,
            xml_escape(options.helper_home.to_string_lossy().as_ref()),
            xml_escape(
                log_dir
                    .join("service.stdout.log")
                    .to_string_lossy()
                    .as_ref()
            ),
            xml_escape(
                log_dir
                    .join("service.stderr.log")
                    .to_string_lossy()
                    .as_ref()
            ),
        )
    }

    fn base_status(
        state: ServiceRuntimeState,
        installed: bool,
        autostart: bool,
        definition: Option<PathBuf>,
    ) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: MACOS_LABEL.to_string(),
            state,
            installed,
            autostart,
            service_definition: definition,
            log_directory: service_log_dir(),
            detail: None,
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub(super) fn install(options: ServiceInstallOptions) -> CliResult<()> {
        let executable = current_executable()?;
        ensure_service_log_dir()?;
        let path = user_unit_path()?;
        let document = render_systemd_unit(&executable, &options);
        write_service_definition(&path, document.as_bytes())?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", LINUX_UNIT_NAME])?;
        if options.start {
            systemctl(&["start", LINUX_UNIT_NAME])?;
        }
        Ok(())
    }

    pub(super) fn uninstall(stop_first: bool) -> CliResult<()> {
        if stop_first {
            let _ = systemctl(&["stop", LINUX_UNIT_NAME]);
        }
        let _ = systemctl(&["disable", LINUX_UNIT_NAME]);
        let path = user_unit_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CliError::Other(format!(
                    "remove systemd user unit {}: {error}",
                    path.display()
                )));
            }
        }
        systemctl(&["daemon-reload"])
    }

    pub(super) fn start() -> CliResult<()> {
        systemctl(&["start", LINUX_UNIT_NAME])
    }

    pub(super) fn stop() -> CliResult<()> {
        systemctl(&["stop", LINUX_UNIT_NAME])
    }

    pub(super) fn status() -> CliResult<ServiceStatus> {
        let path = user_unit_path()?;
        if !path.exists() {
            return Ok(base_status(
                ServiceRuntimeState::NotInstalled,
                false,
                false,
                Some(path),
            ));
        }
        let active = systemctl_output(&["is-active", LINUX_UNIT_NAME]);
        let enabled = systemctl_output(&["is-enabled", LINUX_UNIT_NAME]);
        let state = match active.as_deref() {
            Ok("active") => ServiceRuntimeState::Running,
            Ok("activating") => ServiceRuntimeState::Starting,
            Ok("deactivating") => ServiceRuntimeState::Stopping,
            Ok(_) => ServiceRuntimeState::Stopped,
            Err(_) => ServiceRuntimeState::Unknown,
        };
        let mut status = base_status(
            state,
            true,
            enabled.as_deref().is_ok_and(|value| value == "enabled"),
            Some(path),
        );
        status.detail = active.err().map(|error| error.to_string());
        Ok(status)
    }

    fn systemctl(arguments: &[&str]) -> CliResult<()> {
        systemctl_output(arguments).map(|_| ())
    }

    fn systemctl_output(arguments: &[&str]) -> CliResult<String> {
        let mut args = vec![OsString::from("--user")];
        args.extend(arguments.iter().map(OsString::from));
        run_command("systemctl", &args)
    }

    fn user_unit_path() -> CliResult<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| CliError::Other("resolve user config directory".to_string()))?;
        Ok(config_dir
            .join("systemd")
            .join("user")
            .join(LINUX_UNIT_NAME))
    }

    fn base_status(
        state: ServiceRuntimeState,
        installed: bool,
        autostart: bool,
        definition: Option<PathBuf>,
    ) -> ServiceStatus {
        ServiceStatus {
            platform: ServicePlatform::current(),
            service_name: LINUX_UNIT_NAME.to_string(),
            state,
            installed,
            autostart,
            service_definition: definition,
            log_directory: service_log_dir(),
            detail: None,
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn render_systemd_unit(executable: &Path, options: &ServiceInstallOptions) -> String {
    let service_flag = if options.service_name == "claude" {
        "--claude"
    } else {
        "--codex"
    };
    let helper_home = systemd_environment_assignment("CODEX_HELPER_HOME", &options.helper_home);
    format!(
        "[Unit]\nDescription=codex-helper resident relay\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nEnvironment={helper_home}\nExecStart={} serve {service_flag} --host {} --port {} --no-tui --service-managed\nRestart=on-failure\nRestartSec=10s\nStartLimitIntervalSec=300\nStartLimitBurst=10\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(executable),
        options.host,
        options.port,
    )
}

#[cfg(any(target_os = "linux", test))]
fn systemd_environment_assignment(name: &str, value: &Path) -> String {
    let assignment = format!("{name}={}", value.to_string_lossy());
    format!(
        "\"{}\"",
        assignment.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

fn write_service_definition(path: &Path, contents: &[u8]) -> CliResult<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Other(format!(
            "service definition {} has no parent",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        CliError::Other(format!(
            "create service directory {}: {error}",
            parent.display()
        ))
    })?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, contents).map_err(|error| {
        CliError::Other(format!(
            "write service definition {}: {error}",
            temporary.display()
        ))
    })?;
    std::fs::rename(&temporary, path).map_err(|error| {
        CliError::Other(format!(
            "install service definition {}: {error}",
            path.display()
        ))
    })
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "linux", test))]
fn systemd_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'.'))
    {
        return value.into_owned();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_detection_is_total() {
        assert_ne!(ServicePlatform::current(), ServicePlatform::Unsupported);
    }

    #[test]
    fn service_name_flags_are_unambiguous() {
        assert_eq!(service_name_from_flags(false, false).unwrap(), "codex");
        assert_eq!(service_name_from_flags(true, false).unwrap(), "codex");
        assert_eq!(service_name_from_flags(false, true).unwrap(), "claude");
        assert!(service_name_from_flags(true, true).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_escapes_paths_and_configures_external_restart() {
        let options = ServiceInstallOptions {
            service_name: "codex",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3211,
            start: true,
            helper_home: PathBuf::from("/tmp/helper-home"),
        };
        let document = macos::render_launch_agent(
            Path::new("/tmp/codex & helper"),
            Path::new("/tmp/logs"),
            &options,
        );

        assert!(document.contains("/tmp/codex &amp; helper"));
        assert!(document.contains("<key>KeepAlive</key>"));
        assert!(document.contains("<key>SuccessfulExit</key><false/>"));
        assert!(document.contains("--service-managed"));
    }

    #[test]
    fn systemd_unit_preserves_helper_home_and_runs_as_user() {
        let options = ServiceInstallOptions {
            service_name: "claude",
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3210,
            start: true,
            helper_home: PathBuf::from("/tmp/helper home"),
        };
        let document = render_systemd_unit(Path::new("/usr/bin/codex-helper"), &options);

        assert!(document.contains("Restart=on-failure"));
        assert!(document.contains("WantedBy=default.target"));
        assert!(document.contains("--claude"));
        assert!(document.contains("--service-managed"));
        assert!(document.contains("Environment=\"CODEX_HELPER_HOME=/tmp/helper home\""));
    }
}
