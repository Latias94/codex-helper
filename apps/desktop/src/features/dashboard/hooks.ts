import { useQuery } from "@tanstack/react-query";

import { mapAdminDashboardData } from "@/lib/api/mappers";
import { mockDashboardData } from "@/lib/api/mock-data";
import { queryKeys } from "@/lib/api/query-keys";
import type { QueryBackedData } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";
import { getAppMetadata } from "@/lib/tauri/commands";

export function useAppMetadata() {
  return useQuery({
    queryFn: getAppMetadata,
    queryKey: queryKeys.appMetadata,
  });
}

export function useDashboardData(): QueryBackedData<typeof mockDashboardData> {
  const metadata = useAppMetadata();
  const query = useAdminReadModelState();
  const { readModel, state } = query;

  const appVersion = metadata.data?.version ?? "0.16.0";
  const data = readModel.data
    ? mapAdminDashboardData({
        summary: readModel.data.operatorSummary,
        runtimeStatus: readModel.data.runtimeStatus,
        providers: readModel.data.providers,
        recentRequests: readModel.data.recentRequests,
        usageSummary: readModel.data.usageSummary,
        adminBaseUrl: readModel.data.endpoint.adminBaseUrl,
        appVersion,
      })
    : mockDashboardData;

  return {
    data,
    source: query.source,
    state,
    isLoading: query.isLoading,
    isRefreshing: query.isRefreshing,
    errorMessage: query.errorMessage,
    refetch: query.refetch,
  };
}
