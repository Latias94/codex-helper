import type {
  ApiFinishedRequest,
  ApiAdminReadModelSectionStatus,
  ApiOperatorSummary,
  ApiProviderOption,
  ApiRequestUsageSummaryRow,
  ApiRuntimeStatus,
  ApiUsageDayView,
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
  usageDay?: ApiUsageDayView;
  sectionStatuses: ApiAdminReadModelSectionStatus[];
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
    usageDay: payload.usageDay as ApiUsageDayView | undefined,
    sectionStatuses: (payload.sectionStatuses ?? []) as ApiAdminReadModelSectionStatus[],
  };
}
