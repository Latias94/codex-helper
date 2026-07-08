import { mapUsageData } from "@/lib/api/mappers";
import { mockUsageData } from "@/lib/api/mock-data";
import type { QueryBackedData } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";

export function useUsageData(): QueryBackedData<typeof mockUsageData> {
  const query = useAdminReadModelState();
  const { readModel, state } = query;
  const data = readModel.data
    ? mapUsageData({
        recentRequests: readModel.data.recentRequests,
        usageSummary: readModel.data.usageSummary,
        usageDay: readModel.data.usageDay,
      })
    : mockUsageData;

  return {
    data,
    source: query.source,
    state:
      state.status === "live" && data.summary.totalRows === 0
        ? {
            ...state,
            status: "empty",
            severity: "neutral",
            title: "实时数据已连接，但今天还没有用量",
            description: "先让 Codex 通过本地代理发起一次请求；usage_day 写入后统计和 drilldown 会自动更新。",
            badge: "Empty",
          }
        : state,
    isLoading: query.isLoading,
    isRefreshing: query.isRefreshing,
    errorMessage: query.errorMessage,
    refetch: query.refetch,
  };
}
