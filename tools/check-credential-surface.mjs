import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const failures = [];

const metadata = JSON.parse(
  runCargo(["metadata", "--locked", "--format-version", "1", "--no-deps"]),
);
const core = packageByName(metadata, "codex-helper-core");
const server = packageByName(metadata, "codex-helper-server");
const cli = packageByName(metadata, "codex-helper");

const nativeBackendDependencies = new Map([
  [
    "apple-native-keyring-store",
    {
      target: 'cfg(target_os = "macos")',
      defaultFeatures: false,
      features: ["keychain"],
    },
  ],
  [
    "keyring-core",
    {
      target: 'cfg(any(target_os = "macos", windows))',
      defaultFeatures: true,
      features: [],
    },
  ],
  [
    "secret-service",
    {
      target: 'cfg(target_os = "linux")',
      defaultFeatures: false,
      features: ["rt-tokio-crypto-rust"],
    },
  ],
  [
    "security-framework",
    {
      target: 'cfg(target_os = "macos")',
      defaultFeatures: true,
      features: [],
    },
  ],
  [
    "windows-native-keyring-store",
    {
      target: "cfg(windows)",
      defaultFeatures: false,
      features: [],
    },
  ],
  [
    "zbus",
    {
      target: 'cfg(target_os = "linux")',
      defaultFeatures: false,
      features: [],
    },
  ],
]);

const expectedNativeFeature = [...nativeBackendDependencies.keys()]
  .map((name) => `dep:${name}`)
  .sort();
const actualNativeFeature = [...(core.features["native-credentials"] ?? [])].sort();
if (JSON.stringify(actualNativeFeature) !== JSON.stringify(expectedNativeFeature)) {
  failures.push(
    `codex-helper-core native-credentials feature drifted: ${actualNativeFeature.join(", ")}`,
  );
}

for (const [name, expected] of nativeBackendDependencies) {
  const dependency = core.dependencies.find((candidate) => candidate.name === name);
  if (!dependency) {
    failures.push(`codex-helper-core is missing the approved native dependency ${name}`);
    continue;
  }
  if (!dependency.optional) {
    failures.push(`${name} must remain optional`);
  }
  if (dependency.target !== expected.target) {
    failures.push(`${name} target drifted to ${dependency.target ?? "<all targets>"}`);
  }
  if (dependency.uses_default_features !== expected.defaultFeatures) {
    failures.push(`${name} default-feature policy drifted`);
  }
  if (
    JSON.stringify([...dependency.features].sort()) !==
    JSON.stringify([...expected.features].sort())
  ) {
    failures.push(`${name} feature set drifted: ${dependency.features.join(", ")}`);
  }
}

const suspiciousCredentialDependencies = core.dependencies
  .map((dependency) => dependency.name)
  .filter((name) => /(?:keyring|secret[-_]?service|credential[-_]?store)/i.test(name))
  .filter((name) => !nativeBackendDependencies.has(name));
for (const name of suspiciousCredentialDependencies) {
  failures.push(`unapproved credential-store dependency: ${name}`);
}

const cliCoreDependency = cli.dependencies.find(
  (dependency) => dependency.name === "codex-helper-core",
);
const serverCoreDependency = server.dependencies.find(
  (dependency) => dependency.name === "codex-helper-core",
);
if (!cliCoreDependency?.features.includes("native-credentials")) {
  failures.push("the desktop CLI must explicitly enable native-credentials");
}
if ((serverCoreDependency?.features ?? []).length !== 0) {
  failures.push("codex-helper-server must not enable codex-helper-core features");
}

const isolatedServerTree = runCargo([
  "tree",
  "--locked",
  "-p",
  "codex-helper-server",
  "-e",
  "normal",
  "--target",
  "all",
  "--prefix",
  "none",
  "--format",
  "{p}",
]);
const unifiedWorkspaceTree = runCargo([
  "tree",
  "--locked",
  "--workspace",
  "-e",
  "normal",
  "--target",
  "all",
  "--prefix",
  "none",
  "--format",
  "{p} features=[{f}]",
]);
if (!/^codex-helper-core\s+v[^\n]*features=\[[^\]]*native-credentials[^\]]*\]/m.test(
  unifiedWorkspaceTree,
)) {
  failures.push("workspace-unified dependency graph does not enable core native-credentials");
}
for (const name of nativeBackendDependencies.keys()) {
  if (!new RegExp(`^${escapeRegExp(name)}\\s+v`, "m").test(unifiedWorkspaceTree)) {
    failures.push(`workspace-unified dependency graph is missing selected backend ${name}`);
  }
}
// security-framework is also a transport dependency on macOS, so package presence alone
// cannot identify the credential backend. The store-specific packages remain forbidden.
const serverForbiddenCredentialPackages = [...nativeBackendDependencies.keys()].filter(
  (name) => name !== "security-framework",
);
for (const name of serverForbiddenCredentialPackages) {
  if (new RegExp(`^${escapeRegExp(name)}\\s+v`, "m").test(isolatedServerTree)) {
    failures.push(`isolated codex-helper-server dependency graph contains ${name}`);
  }
}

const serverConfig = read("crates/server/src/config.rs");
const serverCheck = read("crates/server/src/check.rs");
for (const [file, text] of [
  ["crates/server/src/config.rs", serverConfig],
  ["crates/server/src/check.rs", serverCheck],
]) {
  const expectedCapability = file.endsWith("check.rs")
    ? "CredentialSourceCapabilities::server_check()"
    : "CredentialSourceCapabilities::server()";
  if (!text.includes(expectedCapability)) {
    failures.push(`${file} does not declare the native-forbidden server capability`);
  }
  for (const forbidden of ["NativeCredentialStore", "refresh_generation("]) {
    if (text.includes(forbidden)) {
      failures.push(`${file} contains forbidden server credential operation ${forbidden}`);
    }
  }
}
if (!serverConfig.includes("server_runtime_forbids_native_credentials_under_feature_unification")) {
  failures.push("workspace feature-unification server boundary test is missing");
}
const credentialModel = read("crates/core/src/credentials/model.rs");
const credentialTests = read("crates/core/src/credentials/tests.rs");
if (!credentialModel.includes("pub struct SecretValue(Arc<SecretInner>);")) {
  failures.push("SecretValue representation drifted; review the redaction audit");
}
if (!credentialModel.includes("bytes: Zeroizing<Vec<u8>>")) {
  failures.push("SecretValue no longer has an auditable zeroizing byte owner");
}
if (
  !credentialTests.includes(
    "assert_not_impl_any!(SecretValue: std::fmt::Debug, serde::Serialize);",
  )
) {
  failures.push("SecretValue Debug/Serialize compile-time prohibition is missing");
}

const productionCredentialSources = ["crates/core/src", "crates/server/src", "src"]
  .flatMap((root) => collectFiles(path.join(repositoryRoot, root)))
  .filter((file) => !/(?:^|\/)tests?(?:\/|\.rs$)/.test(relative(file)));
const forbiddenProductionPatterns = [
  ["obsolete request-time resolver", /\bresolve_upstream_auth_for_target\b/],
  ["file-backed native credential store", /\b(?:File|Filesystem)CredentialStore\b/],
  ["database-backed native credential store", /\b(?:Database|Sqlite)CredentialStore\b/],
  ["sample keyring adapter", /\b(?:Mock|Sample)CredentialStore\b/],
  ["generic keyring facade", /\bkeyring::Entry\b/],
];
for (const file of productionCredentialSources) {
  const text = fs.readFileSync(file, "utf8");
  for (const [label, pattern] of forbiddenProductionPatterns) {
    if (pattern.test(text)) {
      failures.push(`${relative(file)} contains ${label}`);
    }
  }
}

const routeRegistration = read("crates/core/src/proxy/control_plane_routes/mod.rs");
for (const method of ["connect", "delete", "options", "patch", "post", "put", "trace"]) {
  if (new RegExp(`\\b${method}\\s*\\(`, "i").test(routeRegistration)) {
    failures.push(`remote control plane exposes ${method.toUpperCase()} after credential refactor`);
  }
}

const requiredCanaryContracts = new Map([
  [
    "src/commands/credential.rs",
    "imported_value_never_enters_cli_or_helper_owned_artifacts",
  ],
  ["src/service_receipt.rs", "receipt_bytes_are_non_secret_and_schema_is_closed"],
  [
    "crates/core/src/runtime_host.rs",
    "runtime_config_driver_panic_backtrace_does_not_render_credential_canary",
  ],
  ["crates/core/src/service_status.rs", "provider-body-secret-canary"],
  [
    "crates/core/src/credentials/runtime.rs",
    "generation_captures_sensitive_headers_without_debugging_values",
  ],
]);
for (const [file, marker] of requiredCanaryContracts) {
  if (!read(file).includes(marker)) {
    failures.push(`${file} is missing credential canary contract ${marker}`);
  }
}

const docs = [
  ["README.md", read("README.md")],
  ["README_EN.md", read("README_EN.md")],
  ["docs/CONFIGURATION.md", read("docs/CONFIGURATION.md")],
  ["docs/CONFIGURATION.zh.md", read("docs/CONFIGURATION.zh.md")],
];
for (const [file, text] of docs) {
  if (!text.includes("version = 6")) {
    failures.push(`${file} does not describe the current version 6 runtime contract`);
  }
  if (
    /(?:only supported|only public|remains the only|唯一支持|唯一公开)[^\n]{0,100}`?version = 5`?|`?version = 5`?[^\n]{0,100}(?:only|唯一)/i.test(
      text,
    )
  ) {
    failures.push(`${file} still describes version 5 as the current runtime contract`);
  }
}

const configurationEnglish = docs.find(([file]) => file === "docs/CONFIGURATION.md")[1];
const configurationChinese = docs.find(([file]) => file === "docs/CONFIGURATION.zh.md")[1];
for (const [file, text, required] of [
  [
    "docs/CONFIGURATION.md",
    configurationEnglish,
    [
      "migration never invents a native or secret-file reference",
      "They do not capture arbitrary shell environment variables",
      "opens no runtime store or listener",
      "sends no upstream request",
    ],
  ],
  [
    "docs/CONFIGURATION.zh.md",
    configurationChinese,
    [
      "迁移不会凭空生成 native/secret-file reference",
      "不会捕获任意 shell 环境变量",
      "不打开 runtime store/listener，也不发送任何上游请求",
    ],
  ],
]) {
  for (const marker of required) {
    if (!text.includes(marker)) {
      failures.push(`${file} is missing credential deployment guidance: ${marker}`);
    }
  }
}

for (const [file, marker] of [
  [
    "docs/DOCKER_COMPOSE.md",
    "verify the upstream accepts the new credential",
  ],
  [
    "docs/adr/0001-central-relay-container-runtime.md",
    "Neither placement stores upstream credential values in helper SQLite",
  ],
  [
    "CHANGELOG.md",
    "orphaned logical names require explicit review",
  ],
]) {
  if (!read(file).includes(marker)) {
    failures.push(`${file} is missing the release credential boundary: ${marker}`);
  }
}

const nativeSmoke = read("tools/native-credential-smoke.mjs");
for (const marker of [
  "dedicated runner already has a codex-helper service or receipt",
  "credential canary leaked into helper-owned artifact",
  "initial credential import",
  "import_from_environment_preserves_source",
  "initial_relay_used_imported_credential",
  "rotated_relay_used_recreated_credential",
  "degraded_keeps_service_running",
  "blocked_relay_made_zero_upstream_attempts",
  "explicit_delete_blocks_without_stopping_daemon",
  "cleanup service uninstall",
  "recoverPendingCleanup",
  "stageCandidate",
  "candidate_sha256",
  "native smoke state directory must be outside the Actions checkout",
  "verifyCleanupState",
  "installed service definition is missing or is not a regular file",
  "--self-test",
]) {
  if (!nativeSmoke.includes(marker)) {
    failures.push(`native credential smoke is missing contract marker: ${marker}`);
  }
}

const nativeSmokeWorkflow = read(".github/workflows/native-credential-smoke.yml");
for (const marker of [
  "environment: native-credential-smoke-execution",
  "environment: native-credential-release",
  "verify-release-protection:",
  "rule.prevent_self_review === true",
  "Environment ${environmentName} must configure at least one required reviewer and prevent self-review",
  "Require successful candidate CI",
  "test-build (windows-2025)",
  "windows-credential-manager",
  "macos-keychain",
  "gnome-keyring",
  "kwallet",
  "actions/download-artifact@v8",
  "tools/native-credential-smoke.mjs",
  "Verify artifact run identity",
  "release-prerequisites:",
  "node tools/check-v5-loader-downgrade.mjs",
  "bash tools/docker-mounted-secret-smoke.sh codex-helper-server:release-gate",
  "native-evidence-signoff:",
  "tools/verify-native-credential-evidence.mjs",
]) {
  if (!nativeSmokeWorkflow.includes(marker)) {
    failures.push(`native credential release workflow is missing ${marker}`);
  }
}
if (/\bcargo\s+(?:build|install)\b/.test(nativeSmokeWorkflow)) {
  failures.push("native credential release smoke must consume the current cargo-dist artifact");
}
if (/^    if:/m.test(nativeSmokeWorkflow)) {
  failures.push("native credential release jobs must not have a job-level skip condition");
}

const evidenceVerifier = read("tools/verify-native-credential-evidence.mjs");
for (const marker of [
  '"windows-credential-manager"',
  '"macos-keychain"',
  '"gnome-keyring"',
  '"kwallet"',
  "native-credential-release",
  "verifySidecar",
  "MAX_EVIDENCE_AGE_MS",
  "--self-test",
]) {
  if (!evidenceVerifier.includes(marker)) {
    failures.push(`native credential evidence verifier is missing ${marker}`);
  }
}

const distWorkspace = read("dist-workspace.toml");
if (
  !distWorkspace.includes(
    'global-artifacts-jobs = ["./native-credential-smoke"]',
  )
) {
  failures.push("cargo-dist does not gate host/release on native credential evidence");
}
if (
  !distWorkspace.includes(
    'github-custom-job-permissions = { "native-credential-smoke" = { actions = "read", checks = "read", contents = "read", deployments = "read" } }',
  )
) {
  failures.push("cargo-dist native smoke cannot read current-run artifacts safely");
}
for (const target of [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
]) {
  if (!distWorkspace.includes(target)) {
    failures.push(`cargo-dist native release target is missing: ${target}`);
  }
}

const generatedRelease = read(".github/workflows/release.yml");
for (const marker of [
  "custom-native-credential-smoke:",
  "- build-local-artifacts",
  "- custom-native-credential-smoke",
  "needs.custom-native-credential-smoke.result",
  "needs.plan.outputs.publishing == 'true'",
  "artifacts_matrix.include != null",
  '"actions": "read"',
  '"contents": "read"',
]) {
  if (!generatedRelease.includes(marker)) {
    failures.push(`generated cargo-dist workflow is missing native gate: ${marker}`);
  }
}

const ciWorkflow = read(".github/workflows/ci.yml");
for (const marker of [
  "node tools/check-credential-surface.mjs",
  "node tools/native-credential-smoke.mjs --self-test",
  "node tools/verify-native-credential-evidence.mjs --self-test",
  "tool: cargo-dist@0.32.0",
  "dist generate --mode=ci --check",
  "cargo check --locked -p codex-helper-core --features native-credentials --all-targets",
  "cargo build --locked -p codex-helper-server --target-dir target/server-isolated",
  "test(server_runtime_forbids_native_credentials_under_feature_unification)",
]) {
  if (!ciWorkflow.includes(marker)) {
    failures.push(`cross-platform CI is missing credential gate: ${marker}`);
  }
}

const dockerWorkflow = read(".github/workflows/docker-publish.yml");
if (!dockerWorkflow.includes("node tools/check-credential-surface.mjs")) {
  failures.push("Docker publish does not run the server credential boundary audit");
}
const dockerSmoke = read("tools/docker-mounted-secret-smoke.sh");
for (const marker of [
  "docker-smoke-upstream.mjs",
  "expect_relay_generation initial old",
  "expect_relay_generation restarted new",
  "secret directory ACL does not match root:10001 mode 0750",
]) {
  if (!dockerSmoke.includes(marker)) {
    failures.push(`Docker mounted-secret smoke is missing ${marker}`);
  }
}

if (failures.length > 0) {
  console.error("Credential surface audit failed:");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(
  `Credential surface is closed: ${nativeBackendDependencies.size} approved native dependency declarations, ${serverForbiddenCredentialPackages.length} store-specific packages excluded from the server graph, redaction and canary contracts present.`,
);

function packageByName(cargoMetadata, name) {
  const found = cargoMetadata.packages.find((candidate) => candidate.name === name);
  if (!found) {
    throw new Error(`cargo metadata did not contain ${name}`);
  }
  return found;
}

function runCargo(args) {
  const result = spawnSync("cargo", args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    const detail = [result.stdout, result.stderr].filter(Boolean).join("\n").trim();
    throw new Error(`cargo ${args.join(" ")} failed${detail ? `:\n${detail}` : ""}`);
  }
  return result.stdout;
}

function read(relativePath) {
  return fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
}

function collectFiles(root) {
  if (!fs.existsSync(root)) {
    return [];
  }
  return fs.readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const absolute = path.join(root, entry.name);
    if (entry.isDirectory()) {
      return collectFiles(absolute);
    }
    return entry.isFile() && entry.name.endsWith(".rs") ? [absolute] : [];
  });
}

function relative(file) {
  return path.relative(repositoryRoot, file).split(path.sep).join("/");
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
