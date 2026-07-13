import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const removedPaths = [
  "apps/desktop/src/lib/api/admin-client.ts",
  "crates/core/src/codex_models_cache.rs",
  "crates/core/src/codex_patch_plan.rs",
  "crates/core/src/config_v2.rs",
  "crates/core/src/config_v4.rs",
  "crates/core/src/proxy/attempt_target.rs",
  "crates/core/src/proxy/provider_orchestration.rs",
  "crates/core/src/proxy/request_routing.rs",
  "crates/core/src/proxy/session_overrides.rs",
  "crates/core/src/state/policy_action_store.rs",
  "crates/core/src/state/session_route_ledger.rs",
  "crates/core/src/usage_balance.rs",
  "crates/core/src/usage_forecast.rs",
];

const removedControlPlanePaths = [
  "crates/core/src/proxy/control_plane/capabilities.rs",
  "crates/core/src/proxy/control_plane/codex_capabilities.rs",
  "crates/core/src/proxy/control_plane/codex_live_smoke.rs",
  "crates/core/src/proxy/control_plane/fleet.rs",
  "crates/core/src/proxy/control_plane/session_mutations.rs",
  "crates/core/src/proxy/control_plane/session_observability.rs",
  "crates/core/src/proxy/control_plane_routes/capability_session.rs",
  "crates/core/src/proxy/control_plane_routes/healthchecks.rs",
  "crates/core/src/proxy/control_plane_routes/overrides.rs",
  "crates/core/src/proxy/control_plane_routes/profiles.rs",
  "crates/core/src/proxy/control_plane_routes/providers.rs",
  "crates/core/src/proxy/control_plane_routes/routing.rs",
  "crates/core/src/proxy/control_plane_routes/stations.rs",
  "crates/core/src/proxy/control_plane_routes/status_runtime.rs",
];

const sourceRoots = [
  "apps/desktop/scripts",
  "apps/desktop/src",
  "apps/desktop/src-tauri/src",
  "crates/core/src",
  "crates/server/src",
  "crates/tui/src",
  "src",
];

const sourceFiles = sourceRoots.flatMap((root) =>
  collectSourceFiles(path.join(repositoryRoot, root)),
);
const productionFiles = sourceFiles.filter(
  (file) => !/(?:^|\.)tests?(?:\.|$)|(?:^|\.)spec(?:\.|$)/.test(path.basename(file)),
);
const failures = [];

for (const relativePath of [...removedPaths, ...removedControlPlanePaths]) {
  if (fs.existsSync(path.join(repositoryRoot, relativePath))) {
    failures.push(`${relativePath}: removed compatibility path exists`);
  }
}

const forbiddenIdentifiers = [
  "ConfigV4MigrationReport",
  "ProxyConfigV2",
  "ProxyConfigV4",
  "ServiceViewV2",
  "ServiceViewV4",
  "StationMapping",
];
const forbiddenFieldNames = [
  "cross_station_failover",
  "effective_station",
  "same_station_retry",
  "station_name_filter",
];
const forbiddenProductionStrings = [
  "station_mapping",
  "remote_connections",
];

for (const file of sourceFiles) {
  const relativePath = path.relative(repositoryRoot, file);
  const text = fs.readFileSync(file, "utf8");
  for (const identifier of forbiddenIdentifiers) {
    if (new RegExp(`\\b${identifier}\\b`).test(text)) {
      failures.push(`${relativePath}: contains removed identifier ${identifier}`);
    }
  }
  for (const field of forbiddenFieldNames) {
    const fieldUse = new RegExp(
      `(?:\\.\\s*${field}\\b|\\b${field}\\b\\s*(?::|,|=|\\)|\\())`,
    );
    if (fieldUse.test(text)) {
      failures.push(`${relativePath}: contains removed compatibility field ${field}`);
    }
  }
}

for (const file of productionFiles) {
  const relativePath = path.relative(repositoryRoot, file);
  const text = fs.readFileSync(file, "utf8");
  for (const value of forbiddenProductionStrings) {
    if (text.includes(value)) {
      failures.push(`${relativePath}: contains removed production surface ${value}`);
    }
  }
}

const routeRegistration = readRepositoryFile(
  "crates/core/src/proxy/control_plane_routes/mod.rs",
);
for (const method of ["connect", "delete", "options", "patch", "post", "put", "trace"]) {
  if (new RegExp(`\\b${method}\\s*\\(`, "i").test(routeRegistration)) {
    failures.push(`control-plane route registration contains ${method.toUpperCase()}`);
  }
}

if (failures.length > 0) {
  console.error("Canonical runtime compatibility surface detected:");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(
  `Canonical runtime surface is clean across ${sourceFiles.length} source files and ${removedPaths.length + removedControlPlanePaths.length} removed paths.`,
);

function readRepositoryFile(relativePath) {
  return fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
}

function collectSourceFiles(root) {
  if (!fs.existsSync(root)) {
    return [];
  }
  const files = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const absolute = path.join(root, entry.name);
    if (entry.isDirectory()) {
      if (!["dist", "generated", "node_modules", "target"].includes(entry.name)) {
        files.push(...collectSourceFiles(absolute));
      }
      continue;
    }
    if (entry.isFile() && /\.(?:js|mjs|rs|ts|tsx)$/.test(entry.name)) {
      files.push(absolute);
    }
  }
  return files;
}
