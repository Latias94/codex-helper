import { emptyRuntimeSummary } from "@/lib/api/empty-data";
import { mapRuntimeSummary } from "@/lib/api/mappers";
import type { QueryBackedData, RuntimeSummary } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";
import { useDesktopControlState } from "@/features/runtime/actions";

export function useRuntimeSummary(): QueryBackedData<RuntimeSummary> {
  const query = useAdminReadModelState();
  const control = useDesktopControlState();
  const { facts, response, state } = query;
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
    canUseLiveActions: state.canUseLiveActions && (control.data?.reachable ?? true),
  };
  const data = facts && response
    ? mapRuntimeSummary({
        endpoint: response.endpoint,
        appVersion: "0.20.0",
        recentRequests: facts.recent_requests,
        capturedAtMs: query.model?.captured_at_ms ?? 0,
      })
    : emptyRuntimeSummary(response?.endpoint, "0.20.0", ownerMode);
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
