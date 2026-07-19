#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

import { startCredentialSmokeUpstream } from "./docker-smoke-upstream.mjs";

const SECRET_PREFIX = "codex-helper-native-smoke-secret-";
const COMMAND_TIMEOUT_MS = 60_000;
const STATUS_TIMEOUT_MS = 30_000;

const options = parseArguments(process.argv.slice(2));
if (options.selfTest) {
  await runSelfTest();
} else {
  await runSmoke(options);
}

async function runSmoke({ binary, backend, evidence }) {
  requireDedicatedExecution();
  const sourceExecutable = path.resolve(binary);
  if (!fs.lstatSync(sourceExecutable, { throwIfNoEntry: false })?.isFile()) {
    throw new Error(`candidate binary does not exist or is not a regular file: ${sourceExecutable}`);
  }
  validateBackendForPlatform(backend);

  const stateRoot = prepareStateRoot();
  recoverPendingCleanup(sourceExecutable, backend, stateRoot);
  const root = fs.mkdtempSync(path.join(stateRoot, "run-"));
  const helperHome = path.join(root, "helper");
  const clientHome = path.join(root, "codex");
  fs.mkdirSync(helperHome, { recursive: true });
  fs.mkdirSync(clientHome, { recursive: true });
  const stagedCandidate = stageCandidate(sourceExecutable, root);
  const executable = stagedCandidate.executable;

  const logicalName = `native.smoke.${crypto.randomBytes(10).toString("hex")}`;
  const missingLogicalName = `native.smoke.${crypto.randomBytes(10).toString("hex")}`;
  const importEnvironmentName =
    `CODEX_HELPER_NATIVE_SMOKE_IMPORT_${crypto.randomBytes(10).toString("hex").toUpperCase()}`;
  const initialSecret = `${SECRET_PREFIX}${crypto.randomBytes(32).toString("base64url")}`;
  const rotatedSecret = `${SECRET_PREFIX}${crypto.randomBytes(32).toString("base64url")}`;
  const sensitiveValues = [SECRET_PREFIX, initialSecret, rotatedSecret];
  const commandRecords = [];
  const smokeEnvironment = {
    ...process.env,
    CODEX_HELPER_HOME: helperHome,
    CODEX_HOME: clientHome,
    RUST_BACKTRACE: "full",
    RUST_LOG: "trace",
    [importEnvironmentName]: initialSecret,
  };
  const runnerIdentity = detectRunnerIdentity(backend);
  const candidate = candidateIdentity(executable);
  const proxyPort = await findFreePortPair();
  let upstreamProbe;
  let cleanupJournal;
  let smokeError;
  let cleanupError;
  let successMessage;

  const run = (label, args, settings = {}) => {
    const record = runCandidateCommand(
      executable,
      root,
      smokeEnvironment,
      label,
      args,
      settings,
      sensitiveValues,
    );
    commandRecords.push(record);
    return record;
  };

  try {
    const baseline = parseJson(
      "baseline service status",
      run("baseline service status", ["service", "status", "--json"]).stdout,
    );
    if (baseline.installed || baseline.receipt_state !== "absent") {
      throw new Error(
        "dedicated runner already has a codex-helper service or receipt; refusing to mutate it",
      );
    }

    run("config init", ["config", "init"]);
    cleanupJournal = createCleanupJournal(stateRoot, {
      backend,
      root,
      helperHome,
      clientHome,
      logicalName,
      candidateExecutable: executable,
      candidateSha256: stagedCandidate.sha256,
    });
    const firstCreate = run(
      "initial credential import",
      ["credential", "import", logicalName, "--from-env", importEnvironmentName],
      { allowedExitCodes: [1] },
    );
    if (!combinedOutput(firstCreate).includes("store_committed_runtime_refresh_failed")) {
      throw new Error(
        "initial credential import did not report the required store-committed/runtime-unavailable partial outcome",
      );
    }
    expectEqual(
      smokeEnvironment[importEnvironmentName],
      initialSecret,
      "credential import source environment",
    );

    upstreamProbe = await startCredentialSmokeUpstream({
      credentials: { old: initialSecret, new: rotatedSecret },
    });

    run("provider add", [
      "provider",
      "add",
      "relay",
      "--base-url",
      `http://127.0.0.1:${upstreamProbe.port}/v1`,
    ]);
    run("provider auth binding", [
      "provider",
      "set-auth",
      "relay",
      "--kind",
      "bearer",
      "--native",
      logicalName,
      "--codex",
    ]);
    run("missing provider add", [
      "provider",
      "add",
      "missing",
      "--base-url",
      `http://127.0.0.1:${upstreamProbe.port}/v1`,
      "--disabled",
    ]);
    run("missing provider auth binding", [
      "provider",
      "set-auth",
      "missing",
      "--kind",
      "bearer",
      "--native",
      missingLogicalName,
      "--codex",
    ]);
    const preinstallCredential = credentialStatus(
      run("preinstall credential status", [
        "credential",
        "status",
        logicalName,
        "--json",
      ]).stdout,
      logicalName,
    );
    expectEqual(preinstallCredential.readiness, "ready", "preinstall credential readiness");
    expectEqual(
      preinstallCredential.refresh.status,
      "target_unavailable",
      "preinstall refresh target",
    );

    run(
      "service install",
      [
        "service",
        "install",
        "--codex",
        "--host",
        "127.0.0.1",
        "--port",
        String(proxyPort),
      ],
      { timeoutMs: 90_000 },
    );
    const ready = await pollService(run, "ready");
    validateServiceStatus(ready, "ready");
    const readyCredential = credentialStatus(
      run("ready credential status", [
        "credential",
        "status",
        logicalName,
        "--json",
      ]).stdout,
      logicalName,
    );
    expectEqual(readyCredential.readiness, "ready", "resident credential readiness");
    expectEqual(readyCredential.refresh.status, "runtime_ready", "resident refresh projection");
    await assertRelayGeneration(proxyPort, upstreamProbe, "initial", "old", sensitiveValues);

    run("enable missing provider", ["provider", "enable", "missing", "--codex"]);
    const degraded = await pollService(run, "degraded");
    validateServiceStatus(degraded, "degraded");
    run("disable missing provider", ["provider", "disable", "missing", "--codex"]);
    const readyAfterDegraded = await pollService(run, "ready");
    validateServiceStatus(readyAfterDegraded, "ready");

    run("credential delete", [
      "credential",
      "delete",
      logicalName,
      "--yes",
      "--if-exists",
    ]);
    const blocked = await pollService(run, "blocked");
    validateServiceStatus(blocked, "blocked");
    const missingCredential = credentialStatus(
      run("missing credential status", [
        "credential",
        "status",
        logicalName,
        "--json",
      ]).stdout,
      logicalName,
    );
    expectEqual(missingCredential.readiness, "missing", "deleted credential readiness");
    expectEqual(
      missingCredential.refresh.status,
      "runtime_missing",
      "deleted runtime projection",
    );
    await assertBlockedRelayDoesNotReachUpstream(
      proxyPort,
      upstreamProbe,
      sensitiveValues,
    );

    run(
      "rotated credential create",
      ["credential", "create", logicalName, "--stdin"],
      { input: rotatedSecret },
    );
    const restored = await pollService(run, "ready");
    validateServiceStatus(restored, "ready");
    await assertRelayGeneration(proxyPort, upstreamProbe, "restored", "new", sensitiveValues);
    const unchanged = run(
      "unchanged credential set",
      ["credential", "set", logicalName, "--stdin"],
      { input: rotatedSecret },
    );
    if (!combinedOutput(unchanged).includes("runtime_refresh=unchanged")) {
      throw new Error("same-value native refresh changed the published runtime generation");
    }
    const finalStatus = await pollService(run, "ready");
    validateServiceStatus(finalStatus, "ready");

    await delay(250);
    const definitionPath = requiredServiceDefinitionPath(finalStatus, root);
    const scanRoots = collectScanRoots(root, finalStatus, definitionPath, cleanupJournal);
    const scan = scanArtifacts(scanRoots, sensitiveValues);
    const definition = serviceDefinitionEvidence(definitionPath);
    const completedAt = new Date().toISOString();
    const evidenceDocument = {
      schema_version: 1,
      candidate,
      platform: {
        os: process.platform,
        arch: process.arch,
        release: os.release(),
        backend,
        backend_version: runnerIdentity.backendVersion,
        user_identity: runnerIdentity.userIdentity,
      },
      service: {
        service_name: finalStatus.service_name,
        install_generation: finalStatus.install_generation,
        receipt_state: finalStatus.receipt_state,
        runtime_identity_verified: finalStatus.runtime_identity_verified,
        definition,
      },
      readiness_observations: [
        observation("initial", ready, readyCredential),
        observation("degraded", degraded, null),
        observation("missing", blocked, missingCredential),
        observation("restored", restored, null),
        observation("same_value_refresh", finalStatus, null),
      ],
      failure_matrix: {
        import_from_environment_preserves_source: "passed",
        initial_relay_used_imported_credential: "passed",
        rotated_relay_used_recreated_credential: "passed",
        service_context_ready: "passed",
        degraded_keeps_service_running: "passed",
        blocked_relay_made_zero_upstream_attempts: "passed",
        explicit_delete_blocks_without_stopping_daemon: "passed",
        recreate_restores_service_context: "passed",
        same_value_refresh_avoids_generation_churn: "passed",
        observed_commands_completed_without_prompt_or_timeout: "passed",
        observed_readiness_was_classified: "passed",
      },
      leakage_audit: {
        status: "passed",
        files_scanned: scan.files,
        bytes_scanned: scan.bytes,
        required_roots: [
          "isolated_helper_home",
          "service_log_directory",
          "service_definition",
          "cleanup_journal",
        ],
        surfaces: [
          "command_stdout_stderr",
          "config_and_backups",
          "runtime_sqlite_wal_shm",
          "service_logs",
          "service_definition",
          "operator_and_status_json",
        ],
      },
      release_context: {
        triggered_by: workflowActor(),
        execution_environment:
          process.env.CODEX_HELPER_NATIVE_SMOKE_ENVIRONMENT ??
          "native-credential-smoke-execution",
        workflow_run: workflowRunUrl(),
      },
      completed_at: completedAt,
    };
    writeEvidence(path.resolve(evidence), evidenceDocument, sensitiveValues);
    successMessage =
      `Native credential service smoke passed for ${backend}; evidence=${path.resolve(evidence)}`;
  } catch (error) {
    smokeError = error;
  } finally {
    const cleanupErrors = [];
    let cleanupVerified = false;
    if (cleanupJournal) {
      try {
        const deleted = run(
          "cleanup credential delete",
          ["credential", "delete", logicalName, "--yes", "--if-exists"],
          { allowedExitCodes: [0, 1] },
        );
        if (
          deleted.exitCode !== 0 &&
          !combinedOutput(deleted).includes("store_committed_runtime_refresh_failed")
        ) {
          throw commandFailure("cleanup credential delete", deleted, sensitiveValues);
        }
      } catch (error) {
        cleanupErrors.push(error);
      }
      try {
        run("cleanup service uninstall", ["service", "uninstall"]);
      } catch (error) {
        cleanupErrors.push(error);
      }
      try {
        verifyCleanupState(run, logicalName);
        cleanupVerified = true;
      } catch (error) {
        cleanupErrors.push(error);
      }
    }
    if (!cleanupJournal || cleanupVerified) {
      fs.rmSync(root, { recursive: true, force: true });
      if (cleanupJournal) {
        fs.rmSync(cleanupJournal, { force: true });
      }
    } else {
      console.error(
        `Cleanup incomplete; recovery journal and isolated home retained at ${cleanupJournal}`,
      );
    }
    if (cleanupErrors.length > 0) {
      const detail = cleanupErrors
        .map((error) => sanitize(error.message, sensitiveValues))
        .join("; ");
      cleanupError = new Error(`native smoke cleanup failed: ${detail}`);
    }
    if (upstreamProbe) {
      await upstreamProbe.close();
    }
  }

  if (smokeError && cleanupError) {
    throw new AggregateError(
      [smokeError, cleanupError],
      "native credential smoke and cleanup both failed",
      { cause: smokeError },
    );
  }
  if (smokeError) {
    throw smokeError;
  }
  if (cleanupError) {
    throw cleanupError;
  }
  console.log(successMessage);
}

function parseArguments(args) {
  if (args.length === 1 && args[0] === "--self-test") {
    return { selfTest: true };
  }
  const values = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!["--binary", "--backend", "--evidence"].includes(flag) || value === undefined) {
      usage();
    }
    values.set(flag, value);
  }
  if (values.size !== 3) {
    usage();
  }
  return {
    selfTest: false,
    binary: values.get("--binary"),
    backend: values.get("--backend"),
    evidence: values.get("--evidence"),
  };
}

function usage() {
  throw new Error(
    "usage: native-credential-smoke.mjs --binary PATH --backend windows-credential-manager|macos-keychain|gnome-keyring|kwallet --evidence PATH",
  );
}

function requireDedicatedExecution() {
  if (
    process.env.GITHUB_ACTIONS !== "true" &&
    process.env.CODEX_HELPER_NATIVE_SMOKE_ALLOW_LOCAL !== "1"
  ) {
    throw new Error(
      "real native smoke is restricted to a dedicated CI runner; use --self-test for local validation",
    );
  }
}

function validateBackendForPlatform(backend) {
  const allowed = {
    win32: ["windows-credential-manager"],
    darwin: ["macos-keychain"],
    linux: ["gnome-keyring", "kwallet"],
  }[process.platform];
  if (!allowed?.includes(backend)) {
    throw new Error(`backend ${backend} is not valid on ${process.platform}`);
  }
}

function prepareStateRoot() {
  const configured = process.env.CODEX_HELPER_NATIVE_SMOKE_STATE_DIR;
  const root = path.resolve(
    configured || path.join(os.homedir(), ".codex-helper-native-smoke"),
  );
  fs.mkdirSync(root, { recursive: true, mode: 0o700 });
  if (process.platform !== "win32") {
    fs.chmodSync(root, 0o700);
  }
  const canonicalRoot = fs.realpathSync.native(root);
  const workspace = process.env.GITHUB_WORKSPACE;
  if (workspace) {
    const canonicalWorkspace = fs.realpathSync.native(path.resolve(workspace));
    const relative = path.relative(canonicalWorkspace, canonicalRoot);
    if (relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))) {
      throw new Error("native smoke state directory must be outside the Actions checkout");
    }
  }
  return canonicalRoot;
}

function createCleanupJournal(
  stateRoot,
  {
    backend,
    root,
    helperHome,
    clientHome,
    logicalName,
    candidateExecutable,
    candidateSha256,
  },
) {
  const journalPath = cleanupJournalPath(stateRoot, backend);
  if (fs.existsSync(journalPath)) {
    throw new Error(`pending cleanup journal appeared during smoke setup: ${journalPath}`);
  }
  const document = {
    schema_version: 1,
    backend,
    candidate_executable: candidateExecutable,
    candidate_sha256: candidateSha256,
    root,
    helper_home: helperHome,
    client_home: clientHome,
    logical_name: logicalName,
    created_at: new Date().toISOString(),
  };
  const encoded = `${JSON.stringify(document, null, 2)}\n`;
  assertNoSecret("cleanup journal", encoded, [SECRET_PREFIX]);
  const temporary = `${journalPath}.tmp-${process.pid}`;
  fs.writeFileSync(temporary, encoded, { mode: 0o600, flag: "wx" });
  fs.renameSync(temporary, journalPath);
  return journalPath;
}

function recoverPendingCleanup(sourceExecutable, backend, stateRoot) {
  const journalPath = cleanupJournalPath(stateRoot, backend);
  if (!fs.existsSync(journalPath)) {
    return;
  }
  if (!fs.lstatSync(journalPath).isFile()) {
    throw new Error(`cleanup journal is not a regular file: ${journalPath}`);
  }
  const journal = parseJson("cleanup journal", fs.readFileSync(journalPath, "utf8"));
  validateCleanupJournal(journal, backend, stateRoot);
  ensureRecoveryDirectory(journal.root);
  ensureRecoveryDirectory(journal.helper_home);
  ensureRecoveryDirectory(journal.client_home);
  const executable = recoveryCandidate(sourceExecutable, journal);
  const environment = {
    ...process.env,
    CODEX_HELPER_HOME: journal.helper_home,
    CODEX_HOME: journal.client_home,
    RUST_BACKTRACE: "full",
    RUST_LOG: "trace",
  };
  const run = (label, args, settings = {}) =>
    runCandidateCommand(
      executable,
      journal.root,
      environment,
      label,
      args,
      settings,
      [SECRET_PREFIX],
    );

  try {
    if (!fs.existsSync(path.join(journal.helper_home, "config.toml"))) {
      run("recovery config init", ["config", "init"]);
    }
    const deleted = run(
      "recovery credential delete",
      ["credential", "delete", journal.logical_name, "--yes", "--if-exists"],
      { allowedExitCodes: [0, 1] },
    );
    if (
      deleted.exitCode !== 0 &&
      !combinedOutput(deleted).includes("store_committed_runtime_refresh_failed")
    ) {
      throw commandFailure("recovery credential delete", deleted, [SECRET_PREFIX]);
    }
    run("recovery service uninstall", ["service", "uninstall"]);
    verifyCleanupState(run, journal.logical_name);
  } catch (error) {
    throw new Error(
      `pending native smoke cleanup failed; journal retained at ${journalPath}: ${sanitize(error.message, [SECRET_PREFIX])}`,
      { cause: error },
    );
  }

  fs.rmSync(journal.root, { recursive: true, force: true });
  fs.rmSync(journalPath, { force: true });
  console.log(`Recovered pending native credential smoke cleanup for ${backend}.`);
}

function cleanupJournalPath(stateRoot, backend) {
  return path.join(stateRoot, `cleanup-${backend}.json`);
}

function ensureRecoveryDirectory(directory) {
  const existing = fs.lstatSync(directory, { throwIfNoEntry: false });
  if (existing && !existing.isDirectory()) {
    throw new Error(`recovery path is not a regular directory: ${directory}`);
  }
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
}

function candidateFileName() {
  return process.platform === "win32" ? "codex-helper.exe" : "codex-helper";
}

function stageCandidate(sourceExecutable, root) {
  const directory = path.join(root, "candidate");
  fs.mkdirSync(directory, { recursive: false, mode: 0o700 });
  const executable = path.join(directory, candidateFileName());
  fs.copyFileSync(sourceExecutable, executable, fs.constants.COPYFILE_EXCL);
  if (process.platform !== "win32") {
    fs.chmodSync(executable, 0o700);
  }
  if (!fs.lstatSync(executable).isFile()) {
    throw new Error("staged candidate is not a regular file");
  }
  return { executable, sha256: sha256File(executable) };
}

function recoveryCandidate(sourceExecutable, journal) {
  const executable = path.resolve(journal.candidate_executable);
  const candidate = fs.lstatSync(executable, { throwIfNoEntry: false });
  if (!candidate) {
    return sourceExecutable;
  }
  if (!candidate.isFile()) {
    throw new Error("staged recovery candidate is not a regular file");
  }
  const actualSha256 = sha256File(executable);
  if (actualSha256 !== journal.candidate_sha256) {
    throw new Error("staged recovery candidate digest does not match the cleanup journal");
  }
  return executable;
}

function sha256File(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function validateCleanupJournal(journal, backend, stateRoot) {
  const expectedKeys = [
    "backend",
    "candidate_executable",
    "candidate_sha256",
    "client_home",
    "created_at",
    "helper_home",
    "logical_name",
    "root",
    "schema_version",
  ];
  if (
    !journal ||
    typeof journal !== "object" ||
    Array.isArray(journal) ||
    JSON.stringify(Object.keys(journal).sort()) !== JSON.stringify(expectedKeys)
  ) {
    throw new Error("cleanup journal schema is invalid");
  }
  if (journal.schema_version !== 1 || journal.backend !== backend) {
    throw new Error("cleanup journal version or backend is invalid");
  }
  const root = path.resolve(journal.root);
  const relativeRoot = path.relative(stateRoot, root);
  if (
    !/^run-[^/\\]+$/.test(relativeRoot) ||
    path.dirname(root) !== stateRoot ||
    relativeRoot.startsWith("..") ||
    path.isAbsolute(relativeRoot)
  ) {
    throw new Error("cleanup journal root escapes the native smoke state directory");
  }
  const candidateDirectory = path.join(root, "candidate");
  const candidateExecutable = path.join(candidateDirectory, candidateFileName());
  const candidateDirectoryState = fs.lstatSync(candidateDirectory, {
    throwIfNoEntry: false,
  });
  const candidateState = fs.lstatSync(candidateExecutable, { throwIfNoEntry: false });
  if (
    path.resolve(journal.candidate_executable) !== candidateExecutable ||
    !/^[0-9a-f]{64}$/.test(journal.candidate_sha256) ||
    (candidateDirectoryState && !candidateDirectoryState.isDirectory()) ||
    (candidateState && !candidateState.isFile()) ||
    path.resolve(journal.helper_home) !== path.join(root, "helper") ||
    path.resolve(journal.client_home) !== path.join(root, "codex") ||
    !/^native\.smoke\.[0-9a-f]{20}$/.test(journal.logical_name) ||
    Number.isNaN(Date.parse(journal.created_at))
  ) {
    throw new Error("cleanup journal identity is invalid");
  }
}

function detectRunnerIdentity(backend) {
  if (process.platform === "win32") {
    const identity = runSystem("whoami", ["/user", "/fo", "csv", "/nh"]);
    const sid = identity.match(/S-\d-(?:\d+-)+\d+/)?.[0];
    if (!sid) {
      throw new Error("could not determine the current Windows SID");
    }
    return {
      userIdentity: `sid:${sid}`,
      backendVersion: `Windows ${os.release()} Credential Manager`,
    };
  }
  if (process.platform === "darwin") {
    return {
      userIdentity: `uid:${process.getuid()}`,
      backendVersion: `macOS ${runSystem("sw_vers", ["-productVersion"])} login Keychain`,
    };
  }
  if (!process.env.DBUS_SESSION_BUS_ADDRESS || !process.env.XDG_RUNTIME_DIR) {
    throw new Error("Linux native smoke requires a logged-in user session bus");
  }
  const busStatus = runSystem("busctl", [
    "--user",
    "--no-pager",
    "status",
    "org.freedesktop.secrets",
  ]);
  const expectedMarker = backend === "gnome-keyring" ? /gnome[- ]keyring/i : /kwallet/i;
  if (!expectedMarker.test(busStatus)) {
    throw new Error(`org.freedesktop.secrets is not owned by the expected ${backend} backend`);
  }
  const version =
    backend === "gnome-keyring"
      ? runSystem("gnome-keyring-daemon", ["--version"])
      : firstSuccessfulSystem([
          ["kwalletd6", ["--version"]],
          ["kwalletd5", ["--version"]],
        ]);
  return {
    userIdentity: `uid:${process.getuid()}`,
    backendVersion: firstLine(version),
  };
}

function candidateIdentity(executable) {
  const sha =
    process.env.CODEX_HELPER_CANDIDATE_SHA ??
    process.env.GITHUB_SHA ??
    runSystem("git", ["rev-parse", "HEAD"]);
  if (!/^[0-9a-f]{40}$/i.test(sha)) {
    throw new Error("release candidate SHA is missing or invalid");
  }
  const version = runSystem(executable, ["--version"]);
  const archiveSha256 = process.env.CODEX_HELPER_CANDIDATE_ARCHIVE_SHA256;
  if (!archiveSha256 || !/^[0-9a-f]{64}$/i.test(archiveSha256)) {
    throw new Error("candidate archive SHA-256 is missing or invalid");
  }
  return {
    repository: process.env.GITHUB_REPOSITORY ?? "local",
    sha,
    ref: process.env.CODEX_HELPER_CANDIDATE_REF ?? process.env.GITHUB_REF ?? "local",
    binary_version: firstLine(version),
    archive_sha256: archiveSha256.toLowerCase(),
  };
}

async function findFreePortPair() {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const proxy = await listenOnRandomPort();
    const proxyPort = proxy.address().port;
    const adminPort = proxyPort <= 64_535 ? proxyPort + 1_000 : proxyPort - 1_000;
    try {
      const admin = await listenOnPort(adminPort);
      await closeServer(admin);
      await closeServer(proxy);
      return proxyPort;
    } catch {
      await closeServer(proxy);
    }
  }
  throw new Error("could not reserve a free proxy/admin port pair");
}

function listenOnRandomPort() {
  return listenOnPort(0);
}

function listenOnPort(port) {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen({ host: "127.0.0.1", port, exclusive: true }, () => resolve(server));
  });
}

function closeServer(server) {
  return new Promise((resolve) => server.close(resolve));
}

async function assertRelayGeneration(proxyPort, upstream, phase, expected, sensitiveValues) {
  const probeId = `native-smoke-${phase}-${crypto.randomBytes(8).toString("hex")}`;
  const before = upstream.requestCount();
  const response = await fetch(`http://127.0.0.1:${proxyPort}/v1/responses`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "session_id": `native-smoke-${phase}`,
      "x-codex-helper-smoke-probe": probeId,
    },
    body: JSON.stringify({
      model: "gpt-5",
      input: `native credential ${phase} generation probe`,
      stream: false,
    }),
    signal: AbortSignal.timeout(15_000),
  });
  const body = await response.text();
  assertNoSecret(`${phase} relay response`, body, sensitiveValues);
  expectEqual(response.status, 200, `${phase} relay response status`);
  if (upstream.requestCount() <= before) {
    throw new Error(`${phase} relay did not reach the credential smoke upstream`);
  }
  const records = upstream.records().filter((record) => record.probe_id === probeId);
  expectEqual(records.length, 1, `${phase} relay probe record count`);
  expectEqual(records[0].generation, expected, `${phase} relay credential generation`);
  if (!records[0].path?.split("?", 1)[0].endsWith("/responses")) {
    throw new Error(`${phase} relay probe did not reach the responses endpoint`);
  }
}

async function assertBlockedRelayDoesNotReachUpstream(
  proxyPort,
  upstreamSentinel,
  sensitiveValues,
) {
  await delay(500);
  const connectionsBefore = upstreamSentinel.connectionCount();
  const requestsBefore = upstreamSentinel.requestCount();
  const response = await fetch(`http://127.0.0.1:${proxyPort}/v1/responses`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      model: "gpt-5",
      input: "blocked native credential probe",
      stream: false,
    }),
    signal: AbortSignal.timeout(15_000),
  });
  const body = await response.text();
  assertNoSecret("blocked relay response", body, sensitiveValues);
  if (response.status !== 503) {
    throw new Error(`blocked relay returned ${response.status} instead of 503`);
  }
  await delay(250);
  expectEqual(
    upstreamSentinel.connectionCount(),
    connectionsBefore,
    "blocked relay upstream connection count",
  );
  expectEqual(
    upstreamSentinel.requestCount(),
    requestsBefore,
    "blocked relay upstream request count",
  );
}

async function pollService(run, expectedContext) {
  const deadline = Date.now() + STATUS_TIMEOUT_MS;
  let lastStatus;
  while (Date.now() < deadline) {
    const record = run(`service status ${expectedContext}`, ["service", "status", "--json"]);
    lastStatus = parseJson(`service status ${expectedContext}`, record.stdout);
    if (
      lastStatus.credential_context === expectedContext &&
      lastStatus.runtime_identity_verified === true &&
      lastStatus.receipt_state === "current"
    ) {
      return lastStatus;
    }
    await delay(250);
  }
  throw new Error(
    `service did not reach ${expectedContext}; last state=${lastStatus?.state ?? "unknown"} credential_context=${lastStatus?.credential_context ?? "unknown"}`,
  );
}

function validateServiceStatus(status, expectedContext) {
  if (!status.installed) {
    throw new Error("service status does not report an installed service");
  }
  if (!["running", "starting"].includes(status.state)) {
    throw new Error(`service is not running: ${status.state}`);
  }
  expectEqual(status.receipt_state, "current", "service receipt state");
  expectEqual(status.credential_context, expectedContext, "service credential context");
  expectEqual(status.runtime_identity_verified, true, "service runtime identity");
  if (!/^[0-9a-f]{8}-[0-9a-f-]{27}$/i.test(status.install_generation ?? "")) {
    throw new Error("service install generation is missing or invalid");
  }
}

function credentialStatus(output, logicalName) {
  const payload = parseJson("credential status", output);
  if (payload.schema_version !== 1 || payload.credentials?.length !== 1) {
    throw new Error("credential status returned an unexpected schema or record count");
  }
  const credential = payload.credentials[0];
  expectEqual(credential.reference, `native:${logicalName}`, "credential reference");
  expectEqual(credential.backend, "native", "credential backend");
  return credential;
}

function observation(phase, service, credential) {
  return {
    phase,
    process_state: service.state,
    credential_context: service.credential_context,
    credential_readiness: credential?.readiness ?? service.credential_context,
    runtime_identity_verified: service.runtime_identity_verified,
  };
}

function collectScanRoots(root, status, definitionPath, journalPath) {
  if (typeof status.log_directory !== "string" || status.log_directory.length === 0) {
    throw new Error("service status did not expose its log directory");
  }
  const logDirectory = path.resolve(status.log_directory);
  if (!fs.lstatSync(logDirectory, { throwIfNoEntry: false })?.isDirectory()) {
    throw new Error("service log directory is missing or is not a regular directory");
  }
  if (!fs.lstatSync(journalPath, { throwIfNoEntry: false })?.isFile()) {
    throw new Error("native smoke cleanup journal is missing before leakage audit");
  }
  return [...new Set([root, logDirectory, definitionPath, journalPath])];
}

function scanArtifacts(roots, sensitiveValues) {
  let files = 0;
  let bytes = 0;
  const seen = new Set();
  for (const root of roots) {
    for (const file of collectRegularFiles(root)) {
      const canonical = fs.realpathSync.native(file);
      if (seen.has(canonical)) {
        continue;
      }
      seen.add(canonical);
      const stat = fs.statSync(canonical);
      if (stat.size > 128 * 1024 * 1024) {
        throw new Error(`refusing to skip oversized smoke artifact: ${canonical}`);
      }
      const contents = fs.readFileSync(canonical);
      for (const secret of sensitiveValues) {
        if (contents.indexOf(Buffer.from(secret)) !== -1) {
          throw new Error(`credential canary leaked into helper-owned artifact: ${canonical}`);
        }
      }
      files += 1;
      bytes += contents.length;
    }
  }
  return { files, bytes };
}

function collectRegularFiles(root) {
  const stat = fs.lstatSync(root, { throwIfNoEntry: false });
  if (!stat) {
    return [];
  }
  if (stat.isSymbolicLink()) {
    return [];
  }
  if (stat.isFile()) {
    return [root];
  }
  if (!stat.isDirectory()) {
    return [];
  }
  return fs.readdirSync(root, { withFileTypes: true }).flatMap((entry) =>
    collectRegularFiles(path.join(root, entry.name)),
  );
}

function requiredServiceDefinitionPath(status, root) {
  const candidates = [];
  if (typeof status.service_definition === "string") {
    candidates.push(status.service_definition);
  }
  candidates.push(path.join(root, "helper", "service", "windows-task.xml"));
  const definition = candidates.find((candidate) =>
    fs.lstatSync(candidate, { throwIfNoEntry: false })?.isFile(),
  );
  if (!definition) {
    throw new Error("installed service definition is missing or is not a regular file");
  }
  return path.resolve(definition);
}

function serviceDefinitionEvidence(definition) {
  const bytes = fs.readFileSync(definition);
  return {
    kind: process.platform === "win32" ? "scheduled_task" : "service_manager_file",
    file_name: path.basename(definition),
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
  };
}

function writeEvidence(evidencePath, document, sensitiveValues) {
  const directory = path.dirname(evidencePath);
  fs.mkdirSync(directory, { recursive: true });
  const encoded = `${JSON.stringify(document, null, 2)}\n`;
  assertNoSecret("evidence JSON", encoded, sensitiveValues);
  const temporary = `${evidencePath}.tmp-${process.pid}`;
  fs.writeFileSync(temporary, encoded, { mode: 0o600, flag: "wx" });
  fs.renameSync(temporary, evidencePath);
  const digest = crypto.createHash("sha256").update(encoded).digest("hex");
  fs.writeFileSync(`${evidencePath}.sha256`, `${digest}  ${path.basename(evidencePath)}\n`, {
    mode: 0o600,
  });
}

function commandRecord(label, result) {
  return {
    label,
    exitCode: result.status,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function runCandidateCommand(
  executable,
  cwd,
  environment,
  label,
  args,
  settings,
  sensitiveValues,
) {
  const result = spawnSync(executable, args, {
    cwd,
    env: environment,
    encoding: "utf8",
    input: settings.input,
    maxBuffer: 16 * 1024 * 1024,
    timeout: settings.timeoutMs ?? COMMAND_TIMEOUT_MS,
    windowsHide: true,
  });
  const record = commandRecord(label, result);
  assertNoSecret(`${label} stdout`, record.stdout, sensitiveValues);
  assertNoSecret(`${label} stderr`, record.stderr, sensitiveValues);
  if (result.error) {
    throw new Error(
      `${label} could not complete: ${sanitize(result.error.message, sensitiveValues)}`,
    );
  }
  const allowedExitCodes = settings.allowedExitCodes ?? [0];
  if (!allowedExitCodes.includes(result.status)) {
    throw commandFailure(label, record, sensitiveValues);
  }
  return record;
}

function verifyCleanupState(run, logicalName) {
  const credential = credentialStatus(
    run("verify cleanup credential status", [
      "credential",
      "status",
      logicalName,
      "--json",
    ]).stdout,
    logicalName,
  );
  expectEqual(credential.readiness, "missing", "cleanup credential readiness");
  const service = parseJson(
    "verify cleanup service status",
    run("verify cleanup service status", ["service", "status", "--json"]).stdout,
  );
  expectEqual(service.installed, false, "cleanup service installation state");
  expectEqual(service.receipt_state, "absent", "cleanup service receipt state");
}

function commandFailure(label, record, sensitiveValues) {
  return new Error(
    `${label} exited with ${record.exitCode}: ${sanitize(combinedOutput(record), sensitiveValues)}`,
  );
}

function combinedOutput(record) {
  return [record.stdout, record.stderr].filter(Boolean).join("\n");
}

function assertNoSecret(label, value, sensitiveValues) {
  for (const secret of sensitiveValues) {
    if (String(value).includes(secret)) {
      throw new Error(`${label} contains a credential canary`);
    }
  }
}

function sanitize(value, sensitiveValues) {
  let output = String(value);
  for (const secret of [...sensitiveValues].sort((left, right) => right.length - left.length)) {
    output = output.split(secret).join("[REDACTED]");
  }
  return output.trim();
}

function parseJson(label, value) {
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${label} did not return JSON: ${error.message}`);
  }
}

function expectEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${expected}, got ${actual}`);
  }
}

function runSystem(command, args) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout: COMMAND_TIMEOUT_MS,
    windowsHide: true,
  });
  if (result.error || result.status !== 0) {
    throw new Error(
      `${command} ${args.join(" ")} failed: ${result.error?.message ?? result.stderr ?? result.stdout}`,
    );
  }
  return String(result.stdout || result.stderr).trim();
}

function firstSuccessfulSystem(commands) {
  const failures = [];
  for (const [command, args] of commands) {
    try {
      return runSystem(command, args);
    } catch (error) {
      failures.push(error.message);
    }
  }
  throw new Error(failures.join("; "));
}

function firstLine(value) {
  return String(value).split(/\r?\n/, 1)[0].trim();
}

function workflowActor() {
  const actor =
    process.env.GITHUB_TRIGGERING_ACTOR ??
    process.env.GITHUB_ACTOR ??
    process.env.USERNAME ??
    process.env.USER;
  if (!actor) {
    throw new Error("GitHub workflow actor is missing");
  }
  return actor;
}

function workflowRunUrl() {
  const server = process.env.GITHUB_SERVER_URL;
  const repository = process.env.GITHUB_REPOSITORY;
  const runId = process.env.GITHUB_RUN_ID;
  if (!server || !repository || !runId) {
    if (process.env.CODEX_HELPER_NATIVE_SMOKE_ALLOW_LOCAL === "1") {
      return "local";
    }
    throw new Error("GitHub workflow run identity is missing");
  }
  return `${server}/${repository}/actions/runs/${runId}`;
}

async function runSelfTest() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codex-helper-native-smoke-self-test-"));
  const secret = `${SECRET_PREFIX}${crypto.randomBytes(16).toString("hex")}`;
  const rotated = `${SECRET_PREFIX}${crypto.randomBytes(16).toString("hex")}`;
  let upstream;
  try {
    fs.writeFileSync(path.join(root, "safe"), "redacted\n");
    const safe = scanArtifacts([root], [SECRET_PREFIX, secret]);
    expectEqual(safe.files, 1, "self-test safe file count");
    fs.writeFileSync(path.join(root, "leak"), secret);
    let detected = false;
    try {
      scanArtifacts([root], [SECRET_PREFIX, secret]);
    } catch (error) {
      detected = error.message.includes("credential canary leaked");
    }
    expectEqual(detected, true, "self-test canary detection");
    expectEqual(sanitize(`before ${secret} after`, [SECRET_PREFIX, secret]), "before [REDACTED] after", "self-test sanitization");
    const runRoot = fs.mkdtempSync(path.join(root, "run-"));
    const helperHome = path.join(runRoot, "helper");
    const clientHome = path.join(runRoot, "codex");
    fs.mkdirSync(helperHome);
    fs.mkdirSync(clientHome);
    const sourceCandidate = path.join(root, candidateFileName());
    fs.writeFileSync(sourceCandidate, "candidate bytes\n", { mode: 0o700 });
    const stagedCandidate = stageCandidate(sourceCandidate, runRoot);
    const journalPath = createCleanupJournal(root, {
      backend: "macos-keychain",
      root: runRoot,
      helperHome,
      clientHome,
      logicalName: "native.smoke.0123456789abcdefabcd",
      candidateExecutable: stagedCandidate.executable,
      candidateSha256: stagedCandidate.sha256,
    });
    const journal = parseJson("self-test cleanup journal", fs.readFileSync(journalPath, "utf8"));
    validateCleanupJournal(journal, "macos-keychain", root);
    expectEqual(
      recoveryCandidate(sourceCandidate, journal),
      stagedCandidate.executable,
      "self-test staged recovery candidate",
    );
    fs.appendFileSync(stagedCandidate.executable, "tampered\n");
    let tamperedCandidateRejected = false;
    try {
      recoveryCandidate(sourceCandidate, journal);
    } catch (error) {
      tamperedCandidateRejected = error.message.includes("digest does not match");
    }
    expectEqual(
      tamperedCandidateRejected,
      true,
      "self-test tampered recovery candidate rejection",
    );
    fs.rmSync(stagedCandidate.executable);
    expectEqual(
      recoveryCandidate(sourceCandidate, journal),
      sourceCandidate,
      "self-test missing staged candidate fallback",
    );
    let escapedJournalRejected = false;
    try {
      validateCleanupJournal(
        { ...journal, root: os.tmpdir(), helper_home: path.join(os.tmpdir(), "helper") },
        "macos-keychain",
        root,
      );
    } catch (error) {
      escapedJournalRejected = error.message.includes("escapes");
    }
    expectEqual(escapedJournalRejected, true, "self-test escaping cleanup journal rejection");
    upstream = await startCredentialSmokeUpstream({ credentials: { old: secret, new: rotated } });
    const probeId = "native-smoke-self-test";
    const response = await fetch(`http://127.0.0.1:${upstream.port}/v1/responses`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${secret}`,
        "x-codex-helper-smoke-probe": probeId,
      },
    });
    expectEqual(response.status, 200, "self-test upstream accepted old credential");
    await response.text();
    const record = upstream.records().find((item) => item.probe_id === probeId);
    expectEqual(record?.generation, "old", "self-test upstream credential generation");
    const rotatedProbeId = "native-smoke-self-test-rotated";
    const rotatedResponse = await fetch(`http://127.0.0.1:${upstream.port}/v1/responses`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${rotated}`,
        "x-codex-helper-smoke-probe": rotatedProbeId,
      },
    });
    expectEqual(rotatedResponse.status, 200, "self-test upstream accepted rotated credential");
    await rotatedResponse.text();
    const rotatedRecord = upstream.records().find((item) => item.probe_id === rotatedProbeId);
    expectEqual(rotatedRecord?.generation, "new", "self-test rotated credential generation");
    const rejectedResponse = await fetch(`http://127.0.0.1:${upstream.port}/v1/responses`, {
      method: "POST",
      headers: { authorization: "Bearer unknown" },
    });
    expectEqual(rejectedResponse.status, 401, "self-test upstream rejected unknown credential");
    await rejectedResponse.text();
    console.log("Native credential smoke self-test passed.");
  } finally {
    if (upstream) {
      await upstream.close();
    }
    fs.rmSync(root, { recursive: true, force: true });
  }
}
