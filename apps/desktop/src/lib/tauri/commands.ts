import { invoke } from "@tauri-apps/api/core";

import type { DesktopActionResult, DesktopControlState } from "@/lib/api/types";

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

export type AdminEndpointConfig = {
  proxyPort: number;
  adminPort: number;
  proxyBaseUrl: string;
  adminBaseUrl: string;
};

export type AdminReadModel = {
  endpoint: AdminEndpointConfig;
  operatorSummary: unknown;
  runtimeStatus?: unknown;
  providers: unknown[];
  recentRequests: unknown[];
  usageSummary: unknown[];
};

export type StopProxyScope = "owned" | "attached";
export type CodexPreset = "default" | "chatgpt-bridge" | "imagegen-bridge" | "official-relay" | "official-imagegen";
export type ProviderRuntimeState = "normal" | "draining" | "breaker_open" | "half_open";
export type SessionOverrideDimension =
  | "model"
  | "reasoning_effort"
  | "station_name"
  | "route_target"
  | "service_tier"
  | "all";

export async function getAppMetadata() {
  return invoke<AppMetadata>("get_app_metadata");
}

export async function getKnownPaths() {
  return invoke<KnownPaths>("get_known_paths");
}

export async function getAdminReadModel() {
  return invoke<AdminReadModel>("get_admin_read_model");
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

export async function stopProxy(payload: { scope: StopProxyScope; confirmation: string }) {
  return invoke<DesktopActionResult>("stop_proxy", { payload });
}

export async function switchCodex(payload: {
  enabled: boolean;
  preset?: CodexPreset;
  responsesWebsocket?: boolean;
  confirmation: string;
}) {
  return invoke<DesktopActionResult>("switch_codex", { payload });
}

export async function reloadRuntime() {
  return invoke<DesktopActionResult>("reload_runtime");
}

export async function probeStation(payload: { stationName: string }) {
  return invoke<DesktopActionResult>("probe_station", { payload });
}

export async function refreshProviderBalances(payload: { stationName?: string; providerId?: string } = {}) {
  return invoke<DesktopActionResult>("refresh_provider_balances", { payload });
}

export async function applyProviderRuntimeOverride(payload: {
  providerName: string;
  endpointName?: string;
  enabled?: boolean;
  clearEnabled?: boolean;
  runtimeState?: ProviderRuntimeState;
  clearRuntimeState?: boolean;
}) {
  return invoke<DesktopActionResult>("apply_provider_runtime_override", { payload });
}

export async function setGlobalRouteOverride(payload: { target?: string | null }) {
  return invoke<DesktopActionResult>("set_global_route_override", { payload });
}

export async function applySessionOverrides(payload: {
  sessionId: string;
  model?: string;
  reasoningEffort?: string;
  stationName?: string;
  routeTarget?: string;
  serviceTier?: string;
  clear?: SessionOverrideDimension[];
}) {
  return invoke<DesktopActionResult>("apply_session_overrides", { payload });
}

export async function resetSessionOverrides(payload: { sessionId: string }) {
  return invoke<DesktopActionResult>("reset_session_overrides", { payload });
}
