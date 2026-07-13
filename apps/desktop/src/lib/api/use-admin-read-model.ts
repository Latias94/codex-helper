import { useQuery } from "@tanstack/react-query";

import { buildOperatorReadModelDataState, errorToMessage } from "@/lib/api/data-state";
import { fetchAdminReadModelFromTauri } from "@/lib/api/admin-read-model";
import { queryKeys } from "@/lib/api/query-keys";
import type { RuntimeDataState } from "@/lib/api/types";

export function useAdminReadModelState(options?: { isEmpty?: boolean }) {
  const readModel = useQuery({
    queryFn: fetchAdminReadModelFromTauri,
    queryKey: queryKeys.admin.readModel,
    retry: false,
  });
  const response = readModel.error ? undefined : readModel.data;
  const model = response?.operatorReadModel;
  const state: RuntimeDataState = buildOperatorReadModelDataState({
    model,
    isLoading: readModel.isLoading,
    isFetching: readModel.isFetching,
    error: readModel.error,
    ownerMode: "unknown",
  });
  const facts = model?.status === "ready" || model?.status === "stale" ? model.data : undefined;
  const pageState =
    facts && options?.isEmpty && state.status === "live"
      ? {
          ...state,
          status: "empty" as const,
          severity: "neutral" as const,
          title: "实时数据已连接，但当前没有业务记录",
          description: "当前 coherent read model 中没有可显示的业务记录。",
          badge: "Empty",
        }
      : state;

  return {
    readModel,
    response,
    model,
    facts,
    state: pageState,
    source: pageState.source,
    isLoading: readModel.isLoading,
    isRefreshing: readModel.isFetching && Boolean(facts),
    errorMessage: errorToMessage(readModel.error),
    refetch: () => {
      void readModel.refetch();
    },
  };
}
