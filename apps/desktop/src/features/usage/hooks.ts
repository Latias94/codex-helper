import { emptyUsageData } from "@/lib/api/empty-data";
import { mapUsageData } from "@/lib/api/mappers";
import type { QueryBackedData, UsageData } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";

export function useUsageData(): QueryBackedData<UsageData> {
  const query = useAdminReadModelState();
  const { facts, state } = query;
  const data = facts
      ? mapUsageData({
          recentRequests: facts.recent_requests,
          usageDay: facts.usage_day,
        })
    : emptyUsageData;

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
