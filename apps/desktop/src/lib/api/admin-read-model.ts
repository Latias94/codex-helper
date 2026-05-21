import type {
  ApiFinishedRequest,
  ApiOperatorSummary,
  ApiProviderOption,
  ApiRequestUsageSummaryRow,
  ApiRuntimeStatus,
} from "@/lib/api/admin-types";
import { getAdminReadModel } from "@/lib/tauri/commands";

export type AdminReadModelDto = {
  endpoint: {
    proxyPort: number;
    adminPort: number;
    proxyBaseUrl: string;
    adminBaseUrl: string;
  };
  operatorSummary: ApiOperatorSummary;
  runtimeStatus?: ApiRuntimeStatus;
  providers: ApiProviderOption[];
  recentRequests: ApiFinishedRequest[];
  usageSummary: ApiRequestUsageSummaryRow[];
};

export async function fetchAdminReadModelFromTauri(): Promise<AdminReadModelDto> {
  const payload = await getAdminReadModel();
  return {
    endpoint: payload.endpoint,
    operatorSummary: payload.operatorSummary as ApiOperatorSummary,
    runtimeStatus: payload.runtimeStatus as ApiRuntimeStatus | undefined,
    providers: payload.providers as ApiProviderOption[],
    recentRequests: payload.recentRequests as ApiFinishedRequest[],
    usageSummary: payload.usageSummary as ApiRequestUsageSummaryRow[],
  };
}
