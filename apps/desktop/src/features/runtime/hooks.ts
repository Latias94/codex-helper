import { mapRuntimeSummary } from "@/lib/api/mappers";
import { mockRuntime } from "@/lib/api/mock-data";
import type { QueryBackedData, RuntimeSummary } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";

export function useRuntimeSummary(): QueryBackedData<RuntimeSummary> {
  const query = useAdminReadModelState();
  const { readModel, state } = query;
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
    source: query.source,
    state,
    isLoading: query.isLoading,
    isRefreshing: query.isRefreshing,
    errorMessage: query.errorMessage,
    refetch: query.refetch,
  };
}
