import { useQuery } from "@tanstack/react-query";

import { buildRuntimeDataState, deriveDataSource, errorToMessage } from "@/lib/api/data-state";
import { fetchAdminReadModelFromTauri } from "@/lib/api/admin-read-model";
import { queryKeys } from "@/lib/api/query-keys";
import type { RuntimeDataState } from "@/lib/api/types";

export function useAdminReadModelState(options?: { isEmpty?: boolean }) {
  const readModel = useQuery({
    queryFn: fetchAdminReadModelFromTauri,
    queryKey: queryKeys.admin.readModel,
    retry: false,
  });
  const hasLiveData = Boolean(readModel.data);
  const state: RuntimeDataState = buildRuntimeDataState({
    hasLiveData,
    isLoading: readModel.isLoading,
    isFetching: readModel.isFetching,
    error: readModel.error,
    isEmpty: hasLiveData ? options?.isEmpty : false,
    ownerMode: "unknown",
    lastUpdatedAt: readModel.dataUpdatedAt || undefined,
  });

  return {
    readModel,
    state,
    source: deriveDataSource(state),
    isLoading: readModel.isLoading,
    isRefreshing: readModel.isFetching && hasLiveData,
    errorMessage: errorToMessage(readModel.error),
    refetch: () => {
      void readModel.refetch();
    },
  };
}
