import { useQuery } from "@tanstack/react-query";

import { fetchAdminReadModelFromTauri } from "@/lib/api/admin-read-model";
import { mapRuntimeSummary } from "@/lib/api/mappers";
import { mockRuntime } from "@/lib/api/mock-data";
import { queryKeys } from "@/lib/api/query-keys";
import type { QueryBackedData, RuntimeSummary } from "@/lib/api/types";

export function useRuntimeSummary(): QueryBackedData<RuntimeSummary> {
  const readModel = useQuery({
    queryFn: fetchAdminReadModelFromTauri,
    queryKey: queryKeys.admin.readModel,
    retry: 1,
  });
  const data = readModel.data
    ? mapRuntimeSummary(readModel.data.operatorSummary, {
        adminBaseUrl: readModel.data.endpoint.adminBaseUrl,
        appVersion: "0.16.0",
        runtimeStatus: readModel.data.runtimeStatus,
        recentRequests: readModel.data.recentRequests,
      })
    : mockRuntime;

  return {
    data,
    source: readModel.data ? "live" : "mock",
    isLoading: readModel.isLoading,
    isRefreshing: readModel.isFetching,
    errorMessage: readModel.error instanceof Error ? readModel.error.message : undefined,
    refetch: () => {
      void readModel.refetch();
    },
  };
}
