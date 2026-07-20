import { invoke } from "@tauri-apps/api/core";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";

import type {
  DesktopActionResult,
  DesktopControlState,
  SwitchCodexPayload,
} from "@/lib/api/types";
import type { ApiOperatorReadModel, ApiRequestChainExport } from "@/lib/api/admin-types";

export type AppMetadata = {
  name: string;
  version: string;
  tauri: string;
};

export type KnownPaths = {
  home: string;
  config: string;
  logs: string;
  cache: string;
};

export type KnownPathKind = "home" | "config" | "logs" | "cache";

export type ConfigFileActionResult = {
  ok: boolean;
  action: "export-config";
  message: string;
  source: string;
  destination: string;
  secretWarning: boolean;
};

export type AdminEndpointConfig = {
  proxyPort: number;
  adminPort: number;
  proxyBaseUrl: string;
  adminBaseUrl: string;
};

export type AdminReadModel = {
  endpoint: AdminEndpointConfig;
  operatorReadModel: ApiOperatorReadModel;
};

export type DesktopCommandError = {
  code: string;
  message: string;
  retryable: boolean;
  hint?: string | null;
};

export type RequestChainPayload = {
  traceId?: string;
  requestId?: number;
  session?: string;
  limit?: number;
};

export async function getAppMetadata() {
  return invoke<AppMetadata>("get_app_metadata");
}

export async function showMainWindow() {
  return invoke<void>("show_main_window");
}

export async function hideMainWindow() {
  return invoke<void>("hide_main_window");
}

export async function minimizeMainWindow() {
  return invoke<void>("minimize_main_window");
}

export async function toggleMainWindowMaximized() {
  return invoke<void>("toggle_main_window_maximized");
}

export async function quitApp() {
  return invoke<void>("quit_app");
}

export async function getKnownPaths() {
  return invoke<KnownPaths>("get_known_paths");
}

export async function openKnownPath(payload: { kind: KnownPathKind }) {
  return invoke<void>("open_known_path", { payload });
}

export async function exportConfig(payload: { destination: string }) {
  return invoke<ConfigFileActionResult>("export_config", { payload });
}

export async function getLaunchAtLoginEnabled() {
  return isAutostartEnabled();
}

export async function setLaunchAtLoginEnabled(enabled: boolean) {
  if (enabled) {
    await enableAutostart();
  } else {
    await disableAutostart();
  }
  return isAutostartEnabled();
}

export async function getAdminReadModel() {
  return invoke<AdminReadModel>("get_admin_read_model");
}

export async function getRequestChain(payload: RequestChainPayload) {
  return invoke<ApiRequestChainExport>("get_request_chain", { payload });
}

export async function getDesktopControlState() {
  return invoke<DesktopControlState>("get_desktop_control_state");
}

export async function attachExistingProxy() {
  return invoke<DesktopActionResult>("attach_existing_proxy");
}

export async function startDesktopProxy() {
  return invoke<DesktopActionResult>("start_desktop_proxy");
}

export async function switchCodex(payload: SwitchCodexPayload) {
  return invoke<DesktopActionResult>("switch_codex", { payload });
}
