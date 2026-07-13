import { getAdminReadModel, type AdminReadModel } from "@/lib/tauri/commands";

export type AdminReadModelDto = AdminReadModel;

export async function fetchAdminReadModelFromTauri(): Promise<AdminReadModelDto> {
  return getAdminReadModel();
}
