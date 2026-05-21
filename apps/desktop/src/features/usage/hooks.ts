import { useQuery } from "@tanstack/react-query";

import { fetchAdminReadModelFromTauri } from "@/lib/api/admin-read-model";
import { mapUsageData } from "@/lib/api/mappers";
import { mockUsageData } from "@/lib/api/mock-data";
import { queryKeys } from "@/lib/api/query-keys";
import type { QueryBackedData } from "@/lib/api/types";

export function useUsageData(): QueryBackedData<typeof mockUsageData> {
  const readModel = useQuery({
    queryFn: fetchAdminReadModelFromTauri,
    queryKey: queryKeys.admin.readModel,
    retry: 1,
  });
  const data = readModel.data
    ? mapUsageData({
        recentRequests: readModel.data.recentRequests,
        usageSummary: readModel.data.usageSummary,
      })
    : mockUsageData;

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
