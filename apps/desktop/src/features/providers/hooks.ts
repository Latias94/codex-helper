import { mapProvidersData } from "@/lib/api/mappers";
import { mockProvidersData } from "@/lib/api/mock-data";
import type { QueryBackedData } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";

export function useProvidersData(): QueryBackedData<typeof mockProvidersData> {
  const query = useAdminReadModelState();
  const { readModel, state } = query;
  const data = readModel.data
    ? mapProvidersData(
        readModel.data.operatorSummary,
        readModel.data.providers,
        readModel.data.recentRequests,
      )
    : mockProvidersData;

  return {
    data,
    source: query.source,
    state:
      state.status === "live" && data.providers.length === 0
        ? {
            ...state,
            status: "empty",
            severity: "neutral",
            title: "实时数据已连接，但当前没有供应商",
            description: "先在配置中添加 provider，或检查 /operator/summary 是否加载了 providers。",
            badge: "Empty",
          }
        : state,
    isLoading: query.isLoading,
    isRefreshing: query.isRefreshing,
    errorMessage: query.errorMessage,
    refetch: query.refetch,
  };
}
