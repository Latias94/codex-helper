use std::ffi::OsStr;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT_TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

const CODEX_CONFIG: &[u8] = br#"# preserve this client file byte-for-byte
model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
env_key = "RELAY_API_KEY"
requires_openai_auth = false
"#;

const CODEX_AUTH: &[u8] = br#"{"RELAY_API_KEY":"binary-test-secret-canary"}
"#;

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(label: &str) -> Self {
        let process_id = std::process::id();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let sequence = NEXT_TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex-helper-{label}-{process_id}-{timestamp}-{sequence}"
        ));
        fs::create_dir_all(&path).expect("create isolated process-test directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct ProcessFixture {
    _root: TestDirectory,
    helper_home: PathBuf,
    codex_home: PathBuf,
    codex_config_path: PathBuf,
    codex_auth_path: PathBuf,
}

impl ProcessFixture {
    fn new(label: &str) -> Self {
        let root = TestDirectory::new(label);
        let helper_home = root.path().join("helper");
        let codex_home = root.path().join("codex");
        fs::create_dir_all(&helper_home).expect("create helper home");
        fs::create_dir_all(&codex_home).expect("create Codex home");

        let codex_config_path = codex_home.join("config.toml");
        let codex_auth_path = codex_home.join("auth.json");
        fs::write(&codex_config_path, CODEX_CONFIG).expect("write Codex config fixture");
        fs::write(&codex_auth_path, CODEX_AUTH).expect("write Codex auth fixture");

        Self {
            _root: root,
            helper_home,
            codex_home,
            codex_config_path,
            codex_auth_path,
        }
    }

    fn serve_command(&self, binary: impl AsRef<OsStr>, port: u16) -> Command {
        let port = port.to_string();
        let mut command = Command::new(binary);
        command
            .args([
                "serve",
                "--codex",
                "--host",
                "127.0.0.1",
                "--port",
                port.as_str(),
                "--no-tui",
            ])
            .env("CODEX_HELPER_HOME", &self.helper_home)
            .env("CODEX_HOME", &self.codex_home)
            .env("CODEX_HELPER_TUI_LANG", "en")
            .env("RELAY_API_KEY", "binary-test-runtime-placeholder")
            .env("RUST_LOG", "off")
            .env_remove("CODEX_HELPER_ADMIN_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run_serve(&self, binary: impl AsRef<OsStr>, port: u16) -> Output {
        self.serve_command(binary, port)
            .output()
            .expect("run packaged CLI binary")
    }

    fn spawn_serve(&self, binary: impl AsRef<OsStr>, port: u16) -> Child {
        self.serve_command(binary, port)
            .spawn()
            .expect("spawn packaged ch binary")
    }

    fn run_supervise(&self, binary: impl AsRef<OsStr>, port: u16) -> Output {
        let port = port.to_string();
        Command::new(binary)
            .args([
                "daemon",
                "supervise",
                "--codex",
                "--host",
                "127.0.0.1",
                "--port",
                port.as_str(),
                "--max-restarts",
                "0",
            ])
            .env("CODEX_HELPER_HOME", &self.helper_home)
            .env("CODEX_HOME", &self.codex_home)
            .env("CODEX_HELPER_TUI_LANG", "en")
            .env("RELAY_API_KEY", "binary-test-runtime-placeholder")
            .env("RUST_LOG", "off")
            .env_remove("CODEX_HELPER_ADMIN_TOKEN")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run packaged ch supervisor")
    }

    fn assert_codex_files_unchanged(&self) {
        assert_eq!(
            fs::read(&self.codex_config_path).expect("read Codex config after process exit"),
            CODEX_CONFIG
        );
        assert_eq!(
            fs::read(&self.codex_auth_path).expect("read Codex auth after process exit"),
            CODEX_AUTH
        );
    }

    fn assert_no_switch_journal(&self) {
        assert!(
            !self
                .codex_home
                .join("codex-helper-switch-state.json")
                .exists(),
            "legacy switch state must not be created"
        );

        let state_dir = self.helper_home.join("state");
        let journals = fs::read_dir(&state_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("codex-switch") && name.ends_with(".json"))
            .collect::<Vec<_>>();
        assert!(
            journals.is_empty(),
            "failed startup must not leave an active switch journal: {journals:?}"
        );
    }

    fn legacy_switch_state_path(&self) -> PathBuf {
        self.codex_home.join("codex-helper-switch-state.json")
    }

    fn install_legacy_switch_state(&self, port: u16) {
        let applied = format!(
            r#"# preserve this client file byte-for-byte
model_provider = "codex_proxy"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
env_key = "RELAY_API_KEY"
requires_openai_auth = false

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:{port}/v1"
wire_api = "responses"
request_max_retries = 0
"#
        );
        fs::write(&self.codex_config_path, applied).expect("write legacy applied Codex config");
        fs::write(
            self.legacy_switch_state_path(),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 2,
                "original_config_absent": false,
                "original_model_provider": "relay",
                "original_codex_proxy": null,
                "had_model_providers": true
            }))
            .expect("serialize legacy switch state"),
        )
        .expect("write legacy switch state");
    }
}

struct RunningChild {
    child: Child,
}

impl RunningChild {
    fn new(child: Child) -> Self {
        Self { child }
    }

    fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().expect("inspect packaged ch process")
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn occupied_loopback_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a loopback test port");
    let port = listener
        .local_addr()
        .expect("read reserved loopback address")
        .port();
    (listener, port)
}

fn free_loopback_runtime_port() -> u16 {
    for _ in 0..100 {
        let proxy = TcpListener::bind("127.0.0.1:0").expect("reserve candidate proxy port");
        let port = proxy
            .local_addr()
            .expect("read candidate proxy port")
            .port();
        let admin = codex_helper_core::proxy::admin_loopback_addr_for_proxy_port(port);
        if let Ok(admin_listener) = TcpListener::bind(admin) {
            drop(admin_listener);
            drop(proxy);
            return port;
        }
    }
    panic!("reserve a free proxy/admin port pair");
}

fn wait_for_foreground_listener_then_switch(fixture: &ProcessFixture, port: u16) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut listener_observed = false;
    while Instant::now() < deadline {
        let listening = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(50),
        )
        .is_ok();
        listener_observed |= listening;
        let switched = fs::read_to_string(&fixture.codex_config_path)
            .is_ok_and(|config| config.contains("model_provider = \"codex_proxy\""));
        if switched {
            return listener_observed;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn process_diagnostics(output: &Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn ch_binary_imports_on_first_run_but_does_not_switch_when_bind_fails() {
    let fixture = ProcessFixture::new("ch-bind-failure");
    let (_occupied, port) = occupied_loopback_port();

    let output = fixture.run_serve(env!("CARGO_BIN_EXE_ch"), port);
    assert!(
        !output.status.success(),
        "an occupied listener must fail startup: {}",
        process_diagnostics(&output)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&format!("http://127.0.0.1:{port}")),
        "the process must reach listener binding after onboarding: {}",
        process_diagnostics(&output)
    );

    let helper_config = fs::read_to_string(fixture.helper_home.join("config.toml"))
        .expect("ch first run must persist the imported helper route");
    assert!(helper_config.contains("[codex.providers.relay]"));
    assert!(helper_config.contains("auth_token_env = \"RELAY_API_KEY\""));
    assert!(!helper_config.contains("binary-test-secret-canary"));
    fixture.assert_codex_files_unchanged();
    fixture.assert_no_switch_journal();
}

#[test]
fn ch_binary_switches_after_its_foreground_runtime_is_listening() {
    let fixture = ProcessFixture::new("ch-foreground-switch");
    let port = free_loopback_runtime_port();
    let mut child = RunningChild::new(fixture.spawn_serve(env!("CARGO_BIN_EXE_ch"), port));

    assert!(
        wait_for_foreground_listener_then_switch(&fixture, port),
        "ch must apply its switch only after the foreground runtime is listening; child status={:?}",
        child.try_wait(),
    );
    assert!(
        fixture.helper_home.join("config.toml").exists(),
        "successful ch startup must persist its imported helper route"
    );
}

#[test]
fn ch_binary_recovers_v0203_switch_state_before_first_run_import() {
    let fixture = ProcessFixture::new("ch-legacy-switch-recovery");
    let (_occupied, port) = occupied_loopback_port();
    fixture.install_legacy_switch_state(port);

    let output = fixture.run_serve(env!("CARGO_BIN_EXE_ch"), port);
    assert!(
        !output.status.success(),
        "the occupied listener must still fail after onboarding: {}",
        process_diagnostics(&output)
    );

    let helper_config = fs::read_to_string(fixture.helper_home.join("config.toml"))
        .expect("legacy recovery must continue into first-run import");
    assert!(helper_config.contains("[codex.providers.relay]"));
    assert!(helper_config.contains("auth_token_env = \"RELAY_API_KEY\""));
    assert!(!helper_config.contains("binary-test-secret-canary"));
    fixture.assert_codex_files_unchanged();
    fixture.assert_no_switch_journal();
}

#[test]
fn ch_binary_preserves_broken_v0203_switch_state_and_skips_import() {
    let fixture = ProcessFixture::new("ch-broken-legacy-switch");
    let (_occupied, port) = occupied_loopback_port();
    fixture.install_legacy_switch_state(port);
    let legacy_path = fixture.legacy_switch_state_path();
    let broken_state = b"{ this is not a valid switch state";
    fs::write(&legacy_path, broken_state).expect("replace legacy state with malformed data");
    let config_before = fs::read(&fixture.codex_config_path).expect("read applied config fixture");
    let auth_before = fs::read(&fixture.codex_auth_path).expect("read auth fixture");

    let output = fixture.run_serve(env!("CARGO_BIN_EXE_ch"), port);
    assert!(
        !output.status.success(),
        "malformed recovery authority must fail startup: {}",
        process_diagnostics(&output)
    );
    assert!(
        !fixture.helper_home.join("config.toml").exists(),
        "failed legacy recovery must not persist a helper route"
    );
    assert_eq!(
        fs::read(&fixture.codex_config_path).expect("reread applied config fixture"),
        config_before
    );
    assert_eq!(
        fs::read(&fixture.codex_auth_path).expect("reread auth fixture"),
        auth_before
    );
    assert_eq!(
        fs::read(&legacy_path).expect("reread malformed legacy state"),
        broken_state
    );
}

#[test]
fn ch_supervisor_imports_on_first_run_before_spawning_the_managed_child() {
    let fixture = ProcessFixture::new("ch-supervisor-first-run");
    let (_occupied, port) = occupied_loopback_port();

    let output = fixture.run_supervise(env!("CARGO_BIN_EXE_ch"), port);
    assert!(
        !output.status.success(),
        "the occupied listener must exhaust the zero-restart supervisor: {}",
        process_diagnostics(&output)
    );
    let helper_config = fs::read_to_string(fixture.helper_home.join("config.toml"))
        .expect("the supervisor parent must persist onboarding before spawning its child");
    assert!(helper_config.contains("[codex.providers.relay]"));
    assert!(helper_config.contains("auth_token_env = \"RELAY_API_KEY\""));
    assert!(!helper_config.contains("binary-test-secret-canary"));
    fixture.assert_codex_files_unchanged();
    fixture.assert_no_switch_journal();
}

#[test]
fn codex_helper_binary_does_not_auto_onboard_or_switch() {
    let fixture = ProcessFixture::new("codex-helper-bind-failure");
    let (_occupied, port) = occupied_loopback_port();

    let output = fixture.run_serve(env!("CARGO_BIN_EXE_codex-helper"), port);
    assert!(
        !output.status.success(),
        "an occupied listener must fail startup: {}",
        process_diagnostics(&output)
    );
    assert!(
        !fixture.helper_home.join("config.toml").exists(),
        "ordinary codex-helper startup must not import the Codex client configuration"
    );
    fixture.assert_codex_files_unchanged();
    fixture.assert_no_switch_journal();
}

#[test]
fn codex_helper_supervisor_does_not_auto_onboard_or_switch() {
    let fixture = ProcessFixture::new("codex-helper-supervisor-first-run");
    let (_occupied, port) = occupied_loopback_port();

    let output = fixture.run_supervise(env!("CARGO_BIN_EXE_codex-helper"), port);
    assert!(
        !output.status.success(),
        "the occupied listener must exhaust the zero-restart supervisor: {}",
        process_diagnostics(&output)
    );
    assert!(
        !fixture.helper_home.join("config.toml").exists(),
        "ordinary codex-helper supervision must not import the Codex client configuration"
    );
    fixture.assert_codex_files_unchanged();
    fixture.assert_no_switch_journal();
}
