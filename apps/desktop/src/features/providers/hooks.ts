import { useQuery } from "@tanstack/react-query";

import { fetchAdminReadModelFromTauri } from "@/lib/api/admin-read-model";
import { mapProvidersData } from "@/lib/api/mappers";
import { mockProvidersData } from "@/lib/api/mock-data";
import { queryKeys } from "@/lib/api/query-keys";
import type { QueryBackedData } from "@/lib/api/types";

export function useProvidersData(): QueryBackedData<typeof mockProvidersData> {
  const readModel = useQuery({
    queryFn: fetchAdminReadModelFromTauri,
    queryKey: queryKeys.admin.readModel,
    retry: 1,
  });
  const data = readModel.data
    ? mapProvidersData(
        readModel.data.operatorSummary,
        readModel.data.providers,
        readModel.data.recentRequests,
      )
    : mockProvidersData;

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
