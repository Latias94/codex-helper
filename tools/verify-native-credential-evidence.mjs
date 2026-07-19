#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

const EXPECTED_BACKENDS = new Map([
  ["windows-credential-manager", "win32"],
  ["macos-keychain", "darwin"],
  ["gnome-keyring", "linux"],
  ["kwallet", "linux"],
]);
const MAX_EVIDENCE_AGE_MS = 6 * 60 * 60 * 1_000;

const options = parseArguments(process.argv.slice(2));
if (options.selfTest) {
  runSelfTest();
} else {
  const repository = requireEnvironment("GITHUB_REPOSITORY");
  const serverUrl = requireEnvironment("GITHUB_SERVER_URL").replace(/\/$/, "");
  const workflowRun = workflowRunUrl();
  const manifest = verifyEvidenceDirectory({
    directory: path.resolve(options.directory),
    candidateSha: options.candidateSha.toLowerCase(),
    repository,
    serverUrl,
    workflowRun,
    now: Date.now(),
  });
  writeManifest(path.resolve(options.output), manifest);
  console.log(
    `Verified ${manifest.evidence.length} native credential evidence records for ${manifest.candidate_sha}.`,
  );
}

function verifyEvidenceDirectory({
  directory,
  candidateSha,
  repository,
  serverUrl,
  workflowRun,
  now,
}) {
  if (!/^[0-9a-f]{40}$/.test(candidateSha)) {
    throw new Error("candidate SHA is missing or invalid");
  }
  const evidenceFiles = collectRegularFiles(directory).filter(
    (file) => file.endsWith(".json") && !file.endsWith("signoff.json"),
  );
  if (evidenceFiles.length !== EXPECTED_BACKENDS.size) {
    throw new Error(
      `expected ${EXPECTED_BACKENDS.size} evidence JSON files, found ${evidenceFiles.length}`,
    );
  }

  const records = new Map();
  for (const file of evidenceFiles) {
    const bytes = fs.readFileSync(file);
    verifySidecar(file, bytes);
    const evidence = parseJson(file, bytes.toString("utf8"));
    const backend = evidence.platform?.backend;
    if (!EXPECTED_BACKENDS.has(backend) || records.has(backend)) {
      throw new Error(`evidence backend is missing, unexpected, or duplicated: ${backend}`);
    }
    validateEvidence(evidence, {
      backend,
      expectedOs: EXPECTED_BACKENDS.get(backend),
      candidateSha,
      repository,
      serverUrl,
      workflowRun,
      now,
    });
    records.set(backend, {
      backend,
      evidence_sha256: sha256(bytes),
      archive_sha256: evidence.candidate.archive_sha256,
      install_generation: evidence.service.install_generation,
      completed_at: evidence.completed_at,
      source_workflow_run: evidence.release_context.workflow_run,
    });
  }

  for (const backend of EXPECTED_BACKENDS.keys()) {
    if (!records.has(backend)) {
      throw new Error(`required native backend evidence is missing: ${backend}`);
    }
  }
  return {
    schema_version: 1,
    candidate_sha: candidateSha,
    repository,
    evidence: [...records.values()].sort((left, right) =>
      left.backend.localeCompare(right.backend),
    ),
    approval: {
      environment: "native-credential-release",
      audit_source: "github_environment_deployment_history",
      workflow_run: workflowRun,
    },
    verified_at: new Date(now).toISOString(),
  };
}

function validateEvidence(
  evidence,
  { backend, expectedOs, candidateSha, repository, serverUrl, workflowRun, now },
) {
  expectEqual(evidence.schema_version, 1, `${backend} schema version`);
  expectEqual(evidence.candidate?.repository, repository, `${backend} repository`);
  expectEqual(evidence.candidate?.sha?.toLowerCase(), candidateSha, `${backend} candidate SHA`);
  requirePattern(
    evidence.candidate?.archive_sha256,
    /^[0-9a-f]{64}$/,
    `${backend} archive SHA-256`,
  );
  if (typeof evidence.candidate?.binary_version !== "string" || !evidence.candidate.binary_version) {
    throw new Error(`${backend} binary version is missing`);
  }
  expectEqual(evidence.platform?.os, expectedOs, `${backend} operating system`);
  if (
    typeof evidence.platform?.backend_version !== "string" ||
    !evidence.platform.backend_version ||
    !/^(?:sid:S-|uid:\d+)/i.test(evidence.platform?.user_identity ?? "")
  ) {
    throw new Error(`${backend} backend version or user identity is missing`);
  }

  expectEqual(evidence.service?.receipt_state, "current", `${backend} receipt state`);
  expectEqual(
    evidence.service?.runtime_identity_verified,
    true,
    `${backend} runtime identity`,
  );
  requirePattern(
    evidence.service?.install_generation,
    /^[0-9a-f]{8}-[0-9a-f-]{27}$/i,
    `${backend} install generation`,
  );
  requirePattern(
    evidence.service?.definition?.sha256,
    /^[0-9a-f]{64}$/,
    `${backend} service definition digest`,
  );

  const observations = new Map(
    (evidence.readiness_observations ?? []).map((item) => [item.phase, item]),
  );
  for (const [phase, expected] of [
    ["initial", "ready"],
    ["degraded", "degraded"],
    ["missing", "blocked"],
    ["restored", "ready"],
    ["same_value_refresh", "ready"],
  ]) {
    const observation = observations.get(phase);
    expectEqual(observation?.credential_context, expected, `${backend} ${phase} readiness`);
    expectEqual(observation?.runtime_identity_verified, true, `${backend} ${phase} identity`);
  }

  for (const key of [
    "import_from_environment_preserves_source",
    "initial_relay_used_imported_credential",
    "rotated_relay_used_recreated_credential",
    "service_context_ready",
    "degraded_keeps_service_running",
    "blocked_relay_made_zero_upstream_attempts",
    "explicit_delete_blocks_without_stopping_daemon",
    "recreate_restores_service_context",
    "same_value_refresh_avoids_generation_churn",
    "observed_commands_completed_without_prompt_or_timeout",
    "observed_readiness_was_classified",
  ]) {
    expectEqual(evidence.failure_matrix?.[key], "passed", `${backend} ${key}`);
  }
  expectEqual(evidence.leakage_audit?.status, "passed", `${backend} leakage audit`);
  if (!(evidence.leakage_audit?.files_scanned > 0) || !(evidence.leakage_audit?.bytes_scanned > 0)) {
    throw new Error(`${backend} leakage audit did not scan any bytes`);
  }
  const requiredRoots = new Set(evidence.leakage_audit?.required_roots ?? []);
  for (const root of [
    "isolated_helper_home",
    "service_log_directory",
    "service_definition",
    "cleanup_journal",
  ]) {
    if (!requiredRoots.has(root)) {
      throw new Error(`${backend} leakage audit omitted required root ${root}`);
    }
  }
  expectEqual(
    evidence.release_context?.execution_environment,
    "native-credential-smoke-execution",
    `${backend} execution environment`,
  );
  if (!isWorkflowRunUrl(evidence.release_context?.workflow_run, repository, serverUrl)) {
    throw new Error(`${backend} source workflow run is invalid`);
  }
  expectEqual(
    evidence.release_context.workflow_run,
    workflowRun,
    `${backend} source workflow run`,
  );

  const completedAt = Date.parse(evidence.completed_at);
  if (
    Number.isNaN(completedAt) ||
    completedAt > now + 5 * 60 * 1_000 ||
    now - completedAt > MAX_EVIDENCE_AGE_MS
  ) {
    throw new Error(`${backend} evidence is stale or has an invalid completion time`);
  }
}

function verifySidecar(file, bytes) {
  const sidecar = `${file}.sha256`;
  const text = fs.readFileSync(sidecar, "utf8").trim();
  const match = text.match(/^([0-9a-f]{64})\s{2}([^/\\]+)$/);
  if (!match || match[2] !== path.basename(file) || match[1] !== sha256(bytes)) {
    throw new Error(`evidence checksum sidecar is missing or invalid: ${sidecar}`);
  }
}

function writeManifest(output, manifest) {
  fs.mkdirSync(path.dirname(output), { recursive: true });
  const encoded = `${JSON.stringify(manifest, null, 2)}\n`;
  const temporary = `${output}.tmp-${process.pid}`;
  fs.writeFileSync(temporary, encoded, { mode: 0o600, flag: "wx" });
  fs.renameSync(temporary, output);
  fs.writeFileSync(`${output}.sha256`, `${sha256(Buffer.from(encoded))}  ${path.basename(output)}\n`, {
    mode: 0o600,
  });
}

function collectRegularFiles(root) {
  const stat = fs.lstatSync(root, { throwIfNoEntry: false });
  if (!stat) {
    throw new Error(`evidence directory is missing: ${root}`);
  }
  if (stat.isSymbolicLink()) {
    throw new Error(`evidence path must not be a symbolic link: ${root}`);
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

function parseArguments(args) {
  if (args.length === 1 && args[0] === "--self-test") {
    return { selfTest: true };
  }
  const values = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!["--directory", "--candidate-sha", "--output"].includes(flag) || value === undefined) {
      usage();
    }
    values.set(flag, value);
  }
  if (values.size !== 3) {
    usage();
  }
  return {
    selfTest: false,
    directory: values.get("--directory"),
    candidateSha: values.get("--candidate-sha"),
    output: values.get("--output"),
  };
}

function usage() {
  throw new Error(
    "usage: verify-native-credential-evidence.mjs --directory PATH --candidate-sha SHA --output PATH",
  );
}

function workflowRunUrl() {
  const server = requireEnvironment("GITHUB_SERVER_URL");
  const repository = requireEnvironment("GITHUB_REPOSITORY");
  const runId = requireEnvironment("GITHUB_RUN_ID");
  return `${server}/${repository}/actions/runs/${runId}`;
}

function isWorkflowRunUrl(value, repository, serverUrl) {
  return (
    typeof value === "string" &&
    new RegExp(
      `^${escapeRegExp(serverUrl)}/${escapeRegExp(repository)}/actions/runs/\\d+$`,
    ).test(value)
  );
}

function requireEnvironment(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function requirePattern(value, pattern, label) {
  if (typeof value !== "string" || !pattern.test(value)) {
    throw new Error(`${label} is missing or invalid`);
  }
}

function expectEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${expected}, got ${actual}`);
  }
}

function parseJson(label, value) {
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${label} is not valid JSON: ${error.message}`);
  }
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function runSelfTest() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "native-evidence-self-test-"));
  const candidateSha = "a".repeat(40);
  const repository = "owner/repository";
  const serverUrl = "https://github.com";
  const workflowRun = `${serverUrl}/${repository}/actions/runs/123`;
  const now = Date.now();
  try {
    for (const [index, [backend, platform]] of [...EXPECTED_BACKENDS].entries()) {
      const evidence = selfTestEvidence({
        backend,
        platform,
        candidateSha,
        repository,
        workflowRun,
        now,
        index,
      });
      const file = path.join(root, `${backend}.json`);
      const encoded = Buffer.from(`${JSON.stringify(evidence, null, 2)}\n`);
      fs.writeFileSync(file, encoded);
      fs.writeFileSync(`${file}.sha256`, `${sha256(encoded)}  ${path.basename(file)}\n`);
    }
    const manifest = verifyEvidenceDirectory({
      directory: root,
      candidateSha,
      repository,
      serverUrl,
      workflowRun,
      now,
    });
    expectEqual(manifest.evidence.length, 4, "self-test evidence count");
    fs.writeFileSync(`${path.join(root, "kwallet.json")}.sha256`, `${"0".repeat(64)}  kwallet.json\n`);
    let rejected = false;
    try {
      verifyEvidenceDirectory({
        directory: root,
        candidateSha,
        repository,
        serverUrl,
        workflowRun,
        now,
      });
    } catch (error) {
      rejected = error.message.includes("checksum sidecar");
    }
    expectEqual(rejected, true, "self-test tampered evidence rejection");
    console.log("Native credential evidence verifier self-test passed.");
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

function selfTestEvidence({
  backend,
  platform,
  candidateSha,
  repository,
  workflowRun,
  now,
  index,
}) {
  const observation = (phase, credentialContext) => ({
    phase,
    process_state: "running",
    credential_context: credentialContext,
    credential_readiness: credentialContext,
    runtime_identity_verified: true,
  });
  return {
    schema_version: 1,
    candidate: {
      repository,
      sha: candidateSha,
      ref: "refs/tags/v-test",
      binary_version: "codex-helper test",
      archive_sha256: String(index + 1).repeat(64),
    },
    platform: {
      os: platform,
      arch: "test",
      release: "test",
      backend,
      backend_version: "test backend",
      user_identity: platform === "win32" ? "sid:S-1-5-21-1" : "uid:1000",
    },
    service: {
      service_name: "codex",
      install_generation: "00000000-0000-0000-0000-000000000000",
      receipt_state: "current",
      runtime_identity_verified: true,
      definition: { kind: "test", file_name: "test", sha256: "f".repeat(64) },
    },
    readiness_observations: [
      observation("initial", "ready"),
      observation("degraded", "degraded"),
      observation("missing", "blocked"),
      observation("restored", "ready"),
      observation("same_value_refresh", "ready"),
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
      files_scanned: 1,
      bytes_scanned: 1,
      required_roots: [
        "isolated_helper_home",
        "service_log_directory",
        "service_definition",
        "cleanup_journal",
      ],
    },
    release_context: {
      triggered_by: "test",
      execution_environment: "native-credential-smoke-execution",
      workflow_run: workflowRun,
    },
    completed_at: new Date(now).toISOString(),
  };
}
