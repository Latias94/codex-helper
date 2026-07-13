import { emptyProvidersData } from "@/lib/api/empty-data";
import { mapProvidersData } from "@/lib/api/mappers";
import type { ProvidersData, QueryBackedData } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";

export function useProvidersData(): QueryBackedData<ProvidersData> {
  const query = useAdminReadModelState();
  const { facts, state } = query;
  const data = facts
    ? mapProvidersData(facts.summary)
    : emptyProvidersData;

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
            description: "先在配置中添加 provider，或检查 canonical operator read model 是否包含 providers。",
            badge: "Empty",
          }
        : state,
    isLoading: query.isLoading,
    isRefreshing: query.isRefreshing,
    errorMessage: query.errorMessage,
    refetch: query.refetch,
  };
}
