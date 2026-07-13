import { useQuery } from "@tanstack/react-query";

import { useDesktopControlState } from "@/features/runtime/actions";
import { emptyDashboardData, emptyRuntimeSummary } from "@/lib/api/empty-data";
import { mapAdminDashboardData } from "@/lib/api/mappers";
import { queryKeys } from "@/lib/api/query-keys";
import type { DashboardData, QueryBackedData } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";
import { getAppMetadata } from "@/lib/tauri/commands";

export function useAppMetadata() {
  return useQuery({
    queryFn: getAppMetadata,
    queryKey: queryKeys.appMetadata,
  });
}

export function useDashboardData(): QueryBackedData<DashboardData> {
  const metadata = useAppMetadata();
  const control = useDesktopControlState();
  const query = useAdminReadModelState();
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

  const appVersion = metadata.data?.version ?? "0.20.0";
  const data = facts && response
    ? mapAdminDashboardData({
        summary: facts.summary,
        recentRequests: facts.recent_requests,
        usageDay: facts.usage_day,
        endpoint: response.endpoint,
        appVersion,
        capturedAtMs: query.model?.captured_at_ms ?? 0,
      })
    : emptyDashboardData(emptyRuntimeSummary(response?.endpoint, appVersion, ownerMode));
  const dataWithOwner = {
    ...data,
    runtime: {
      ...data.runtime,
      ownerMode,
    },
  };

  return {
    data: dataWithOwner,
    source: query.source,
    state: mergedState,
    isLoading: query.isLoading,
    isRefreshing: query.isRefreshing || control.isFetching,
    errorMessage: query.errorMessage,
    refetch: query.refetch,
  };
}
