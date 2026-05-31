import { mapRuntimeSummary } from "@/lib/api/mappers";
import { mockRuntime } from "@/lib/api/mock-data";
import type { QueryBackedData, RuntimeSummary } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";
import { useDesktopControlState } from "@/features/runtime/actions";

export function useRuntimeSummary(): QueryBackedData<RuntimeSummary> {
  const query = useAdminReadModelState();
  const control = useDesktopControlState();
  const { readModel, state } = query;
  const ownerMode = control.data
    ? control.data.connectionMode === "desktop-owned"
      ? "desktop-owned"
      : control.data.connectionMode === "attached"
        ? "attached"
        : "unknown"
    : state.ownerMode;
  const mergedState = {
    ...state,
    ownerMode,
    canStartProxy: control.data?.canStart ?? state.canStartProxy,
    canAttachProxy: control.data?.canAttach ?? state.canAttachProxy,
    canStopProxy: control.data?.canStopOwned ?? state.canStopProxy,
    canUseLiveActions: state.canUseLiveActions && (control.data?.reachable ?? true),
  };
  const data = readModel.data
    ? mapRuntimeSummary(readModel.data.operatorSummary, {
        adminBaseUrl: readModel.data.endpoint.adminBaseUrl,
        appVersion: "0.18.0",
        runtimeStatus: readModel.data.runtimeStatus,
        recentRequests: readModel.data.recentRequests,
      })
    : mockRuntime;
  const runtime = {
    ...data,
    ownerMode,
  };

  return {
    data: runtime,
    source: query.source,
    state: mergedState,
    isLoading: query.isLoading,
    isRefreshing: query.isRefreshing || control.isFetching,
    errorMessage: query.errorMessage,
    refetch: query.refetch,
  };
}
