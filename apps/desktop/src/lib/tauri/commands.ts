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

export async function getAppMetadata() {
  return invoke<AppMetadata>("get_app_metadata");
}

export async function getKnownPaths() {
  return invoke<KnownPaths>("get_known_paths");
}
