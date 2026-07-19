import fs from "node:fs";
import path from "node:path";

import { desktopRoot } from "./desktop-contracts.mjs";

const sourceRoots = ["src-tauri/src", "src"];
const sourceFiles = sourceRoots.flatMap((root) => collectSourceFiles(path.join(desktopRoot, root)));
const sources = sourceFiles.map((file) => ({
  file: path.relative(desktopRoot, file),
  text: fs.readFileSync(file, "utf8"),
}));

const forbiddenRemotePaths = [
  "/__codex_helper/api/v1/runtime/reload",
  "/__codex_helper/api/v1/runtime/shutdown",
  "/__codex_helper/api/v1/stations/probe",
  "/__codex_helper/api/v1/providers/balances/refresh",
  "/__codex_helper/api/v1/providers/runtime",
  "/__codex_helper/api/v1/overrides/global-route",
  "/__codex_helper/api/v1/overrides/session",
  "/__codex_helper/api/v1/overrides/session/reset",
];

const forbiddenTauriCommands = [
  "import_config",
  "save_common_provider",
  "reload_runtime",
  "stop_proxy",
  "probe_station",
  "refresh_provider_balances",
  "apply_provider_runtime_override",
  "set_global_route_override",
  "apply_session_overrides",
  "reset_session_overrides",
];

const forbiddenFrontendSymbols = [
  "configured_active_station",
  "effective_active_station",
  "global_station_override",
  "override_station_name",
  "importConfig",
  "saveCommonProvider",
  "ImportConfigPayload",
  "ProviderCommonEditPayload",
  "ProviderConfigEditResult",
  "providerCommonEditSchema",
  "onSaveCommonEdit",
  "添加供应商",
  "高级路由设置",
  "导入配置",
  "reloadRuntime",
  "stopProxy",
  "probeStation",
  "refreshProviderBalances",
  "applyProviderRuntimeOverride",
  "setGlobalRouteOverride",
  "applySessionOverrides",
  "resetSessionOverrides",
  "refreshBalances",
  "setProviderOverride",
  "setGlobalRoute",
  "setSessionOverrides",
  "resetSession",
  "stopOwned",
  "stopAttached",
  "shutdown_available",
  "shutdownAvailable",
  "canStopOwned",
  "canRemoteStop",
  "canStopProxy",
];

const forbiddenRustWriteMethods = [
  /\.\s*(?:post|put|patch|delete|options|connect|trace)\s*\(/i,
  /\.\s*request\s*\(/,
  /\bMethod::(?!(?:GET|HEAD)\b)[A-Z_]+\b/,
  /\bMethod::(?:from_bytes|from_str)\b/,
];

const forbiddenFrontendWriteMethods = [
  /\bmethod\s*:\s*["'`](?!(?:GET|HEAD)["'`])[^"'`]+["'`]/i,
];

const allowedReadOnlyCapabilityFields = new Map([
  [
    "src/lib/api/admin-types.ts",
    new Map([
      ["refresh_provider_balances", /^\s{2}refresh_provider_balances: boolean;$/m],
    ]),
  ],
]);

const failures = [];

for (const source of sources) {
  const writeMethodPatterns = source.file.startsWith("src-tauri/")
    ? forbiddenRustWriteMethods
    : forbiddenFrontendWriteMethods;
  if (writeMethodPatterns.some((pattern) => pattern.test(source.text))) {
    failures.push(`${source.file}: contains an outbound HTTP write method`);
  }
  for (const value of [
    ...forbiddenRemotePaths,
    ...forbiddenTauriCommands,
    ...forbiddenFrontendSymbols,
  ]) {
    if (source.text.includes(value) && !isAllowedReadOnlyCapabilityField(source, value)) {
      failures.push(`${source.file}: contains forbidden remote mutation surface ${JSON.stringify(value)}`);
    }
  }
}

if (failures.length > 0) {
  console.error("Desktop control-plane mutation surface detected:");
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Desktop control-plane surface is read-only across ${sourceFiles.length} production files.`);

function isAllowedReadOnlyCapabilityField(source, value) {
  const declaration = allowedReadOnlyCapabilityFields.get(source.file)?.get(value);
  if (!declaration) {
    return false;
  }
  const occurrences = source.text.split(value).length - 1;
  return occurrences === 1 && declaration.test(source.text);
}

function collectSourceFiles(root) {
  const files = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const absolute = path.join(root, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "generated") {
        files.push(...collectSourceFiles(absolute));
      }
      continue;
    }
    if (!entry.isFile() || !/\.(?:rs|ts|tsx)$/.test(entry.name) || /\.test\.[^.]+$/.test(entry.name)) {
      continue;
    }
    files.push(absolute);
  }
  return files;
}
