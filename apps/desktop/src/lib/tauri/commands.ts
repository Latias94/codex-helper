import { invoke } from "@tauri-apps/api/core";

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

export async function getAppMetadata() {
  return invoke<AppMetadata>("get_app_metadata");
}

export async function getKnownPaths() {
  return invoke<KnownPaths>("get_known_paths");
}

export async function getAdminReadModel() {
  return invoke<AdminReadModel>("get_admin_read_model");
}
