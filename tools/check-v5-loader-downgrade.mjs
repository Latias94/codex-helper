import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const FROZEN_TAG = "v0.20.3";
const FROZEN_COMMIT = "6a9600ffd63870807d25c4ae118d47533a9c1eed";
const FROZEN_TREE = "d637bf5522a930cbef720dff98d8493ba53260e3";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const temporaryRoot = fs.mkdtempSync(path.join(os.tmpdir(), "codex-helper-v5-loader-"));

try {
  verifyFrozenRevision();

  const archivePath = path.join(temporaryRoot, "v0.20.3-core.tar");
  const sourceRoot = path.join(temporaryRoot, "source");
  fs.mkdirSync(sourceRoot);
  run("git", [
    "archive",
    "--format=tar",
    `--output=${archivePath}`,
    FROZEN_COMMIT,
  ]);
  run("tar", ["-xf", archivePath, "-C", sourceRoot]);

  const frozenCore = path.join(sourceRoot, "crates", "core");
  installGateHarness(frozenCore);

  const cargo = process.env.CARGO?.trim() || "cargo";
  const targetDirectory = path.join(repositoryRoot, "target", "frozen-v5-loader");
  run(cargo, [
    "test",
    "--locked",
    "--offline",
    "--manifest-path",
    path.join(sourceRoot, "Cargo.toml"),
    "--package",
    "codex-helper-core",
    "--target-dir",
    targetDirectory,
    "--test",
    "frozen_v5_loader",
  ]);

  console.log(`Verified the real ${FROZEN_TAG} loader at ${FROZEN_COMMIT}.`);
} finally {
  fs.rmSync(temporaryRoot, { recursive: true, force: true });
}

function verifyFrozenRevision() {
  const commit = captureGitRevision(`${FROZEN_COMMIT}^{commit}`);
  if (commit !== FROZEN_COMMIT) {
    throw new Error(`Expected frozen commit ${FROZEN_COMMIT}, found ${commit}.`);
  }

  const tagCommit = captureGitRevision(`${FROZEN_TAG}^{commit}`);
  if (tagCommit !== FROZEN_COMMIT) {
    throw new Error(
      `${FROZEN_TAG} resolves to ${tagCommit}, expected ${FROZEN_COMMIT}.`,
    );
  }

  const tree = captureGitRevision(`${FROZEN_COMMIT}^{tree}`);
  if (tree !== FROZEN_TREE) {
    throw new Error(`Frozen commit tree is ${tree}, expected ${FROZEN_TREE}.`);
  }
}

function captureGitRevision(revision) {
  const result = spawnSync("git", ["rev-parse", "--verify", revision], {
    cwd: repositoryRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    throw new Error(
      `Cannot resolve ${revision}. The v0.20.3 downgrade gate requires repository history and never fetches it at runtime. Use a non-shallow checkout (GitHub Actions: fetch-depth: 0).\n${result.stderr.trim()}`,
    );
  }
  return result.stdout.trim();
}

function installGateHarness(frozenCore) {
  const testsDirectory = path.join(frozenCore, "tests");
  const fixturesDirectory = path.join(testsDirectory, "fixtures");
  fs.mkdirSync(fixturesDirectory, { recursive: true });

  for (const fixture of [
    "pre-v6-legacy.json",
    "pre-v6-unversioned.toml",
    "version-5-credential-compat.toml",
    "version-6-downgrade-boundary.toml",
  ]) {
    fs.copyFileSync(
      path.join(
        repositoryRoot,
        "crates",
        "core",
        "src",
        "config",
        "tests",
        "fixtures",
        fixture,
      ),
      path.join(fixturesDirectory, fixture),
    );
  }

  fs.writeFileSync(path.join(testsDirectory, "frozen_v5_loader.rs"), gateHarness());
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    stdio: "inherit",
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}.`);
  }
}

function gateHarness() {
  return String.raw`use codex_helper_core::config::{ProxyConfig, load_config, save_config};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION_5: &str = include_str!("fixtures/version-5-credential-compat.toml");
const VERSION_6: &str = include_str!("fixtures/version-6-downgrade-boundary.toml");
const UNVERSIONED: &str = include_str!("fixtures/pre-v6-unversioned.toml");
const LEGACY_JSON: &str = include_str!("fixtures/pre-v6-legacy.json");

struct IsolatedHelperHome {
    root: PathBuf,
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl IsolatedHelperHome {
    fn create() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-real-v5-loader-{}-{nonce}",
            std::process::id()
        ));
        let helper_home = root.join(".codex-helper");
        std::fs::create_dir_all(&helper_home).expect("create isolated helper home");
        let overrides = [
            ("CODEX_HELPER_HOME", helper_home.into_os_string()),
            ("FIXTURE_INLINE_ENV", OsString::from("fixture-env-token")),
            ("FIXTURE_RELAY_TOKEN", OsString::from("fixture-relay-token")),
            ("FIXTURE_CLAUDE_KEY", OsString::from("fixture-claude-key")),
        ];
        let previous = overrides
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(*key)))
            .collect();
        for (key, value) in overrides {
            unsafe { std::env::set_var(key, value) };
        }
        Self { root, previous }
    }

    fn config_path(&self) -> PathBuf {
        self.root.join(".codex-helper").join("config.toml")
    }

    fn json_config_path(&self) -> PathBuf {
        self.root.join(".codex-helper").join("config.json")
    }
}

impl Drop for IsolatedHelperHome {
    fn drop(&mut self) {
        for (key, previous) in self.previous.drain(..).rev() {
            unsafe {
                match previous {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, contents: &[u8]) {
    std::fs::write(path, contents).expect("write isolated config fixture");
}

#[test]
fn real_v0203_loader_accepts_v5_and_rejects_v6_without_writing() {
    let home = IsolatedHelperHome::create();
    let config_path = home.config_path();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build Tokio runtime");

    write(&config_path, VERSION_5.as_bytes());
    let loaded = runtime
        .block_on(load_config())
        .expect("v0.20.3 loader must accept the version 5 fixture");
    assert_eq!(loaded.version, Some(5));

    let codex = loaded.codex.active_station().expect("active Codex route");
    let inline = codex
        .upstreams
        .iter()
        .find(|upstream| upstream.base_url == "https://inline.example/v1")
        .expect("inline credential provider");
    assert_eq!(inline.auth.auth_token.as_deref(), Some("fixture-inline-token"));
    assert_eq!(inline.auth.auth_token_env.as_deref(), Some("FIXTURE_INLINE_ENV"));
    assert_eq!(
        inline.auth.resolve_auth_token().as_deref(),
        Some("fixture-inline-token")
    );
    let environment = codex
        .upstreams
        .iter()
        .find(|upstream| upstream.base_url == "https://environment.example/v1")
        .expect("environment credential provider");
    assert!(environment.auth.auth_token.is_none());
    assert_eq!(
        environment.auth.auth_token_env.as_deref(),
        Some("FIXTURE_RELAY_TOKEN")
    );
    assert_eq!(
        environment.auth.resolve_auth_token().as_deref(),
        Some("fixture-relay-token")
    );

    let claude = loaded.claude.active_station().expect("active Claude route");
    let api_key = claude
        .upstreams
        .iter()
        .find(|upstream| upstream.base_url == "https://claude.example/v1")
        .expect("Claude environment credential provider");
    assert!(api_key.auth.api_key.is_none());
    assert_eq!(api_key.auth.api_key_env.as_deref(), Some("FIXTURE_CLAUDE_KEY"));
    assert_eq!(
        api_key.auth.resolve_api_key().as_deref(),
        Some("fixture-claude-key")
    );

    assert_eq!(
        std::fs::read(&config_path).expect("read unchanged version 5 source"),
        VERSION_5.as_bytes()
    );
    assert!(!config_path.with_file_name("config.toml.bak").exists());

    std::fs::remove_file(&config_path).expect("remove version 5 fixture");
    write(&config_path, UNVERSIONED.as_bytes());
    let restored_unversioned: ProxyConfig =
        toml::from_str(UNVERSIONED).expect("v0.20.3 parser must accept unversioned TOML");
    runtime
        .block_on(save_config(&restored_unversioned))
        .expect("v0.20.3 migrator must restore unversioned TOML");
    assert_eq!(
        std::fs::read(config_path.with_file_name("config.toml.bak"))
            .expect("read v0.20.3 unversioned backup"),
        UNVERSIONED.as_bytes()
    );
    let loaded = runtime
        .block_on(load_config())
        .expect("v0.20.3 loader must start after restoring unversioned TOML");
    let upstream = loaded
        .codex
        .active_station()
        .expect("active unversioned Codex route")
        .upstreams
        .first()
        .expect("unversioned Codex upstream");
    assert_eq!(
        upstream.auth.auth_token.as_deref(),
        Some("fixture-unversioned-inline")
    );
    assert_eq!(
        upstream.auth.resolve_auth_token().as_deref(),
        Some("fixture-unversioned-inline")
    );

    std::fs::remove_file(&config_path).expect("remove restored unversioned config");
    std::fs::remove_file(config_path.with_file_name("config.toml.bak"))
        .expect("remove unversioned backup");
    let json_path = home.json_config_path();
    write(&json_path, LEGACY_JSON.as_bytes());
    let restored_json: ProxyConfig =
        serde_json::from_str(LEGACY_JSON).expect("v0.20.3 parser must accept legacy JSON");
    runtime
        .block_on(save_config(&restored_json))
        .expect("v0.20.3 migrator must restore legacy JSON");
    assert_eq!(
        std::fs::read(json_path.with_file_name("config.json.bak"))
            .expect("read v0.20.3 legacy JSON backup"),
        LEGACY_JSON.as_bytes()
    );
    let loaded = runtime
        .block_on(load_config())
        .expect("v0.20.3 loader must start after restoring legacy JSON");
    let upstream = loaded
        .codex
        .active_station()
        .expect("active legacy JSON Codex route")
        .upstreams
        .first()
        .expect("legacy JSON Codex upstream");
    assert_eq!(
        upstream.auth.auth_token_env.as_deref(),
        Some("FIXTURE_RELAY_TOKEN")
    );
    assert_eq!(
        upstream.auth.resolve_auth_token().as_deref(),
        Some("fixture-relay-token")
    );

    std::fs::remove_file(&config_path).expect("remove restored legacy JSON config");
    std::fs::remove_file(&json_path).expect("remove legacy JSON source");
    std::fs::remove_file(json_path.with_file_name("config.json.bak"))
        .expect("remove legacy JSON backup");

    write(&config_path, VERSION_6.as_bytes());
    let backup_path = config_path.with_file_name("config.toml.bak");
    let sentinel_backup: &[u8] = b"existing-backup-must-not-change";
    write(&backup_path, sentinel_backup);
    let source_before = std::fs::read(&config_path).expect("read version 6 source");

    let error = runtime
        .block_on(load_config())
        .expect_err("v0.20.3 loader must reject version 6");
    let message = format!("{error:#}");
    assert!(message.contains("config schema 6"), "unexpected error: {message}");
    assert!(
        message.contains("normal startup only accepts version = 5"),
        "unexpected error: {message}"
    );
    assert_eq!(
        std::fs::read(&config_path).expect("read unchanged version 6 source"),
        source_before
    );
    assert_eq!(
        std::fs::read(&backup_path).expect("read unchanged backup"),
        sentinel_backup
    );
}
`;
}
