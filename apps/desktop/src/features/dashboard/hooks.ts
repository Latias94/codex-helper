import { useQuery } from "@tanstack/react-query";

import { mapAdminDashboardData } from "@/lib/api/mappers";
import { mockDashboardData } from "@/lib/api/mock-data";
import { queryKeys } from "@/lib/api/query-keys";
import type { QueryBackedData } from "@/lib/api/types";
import { useAdminReadModelState } from "@/lib/api/use-admin-read-model";
import { getAppMetadata } from "@/lib/tauri/commands";
import { useDesktopControlState } from "@/features/runtime/actions";

export function useAppMetadata() {
  return useQuery({
    queryFn: getAppMetadata,
    queryKey: queryKeys.appMetadata,
  });
}

export function useDashboardData(): QueryBackedData<typeof mockDashboardData> {
  const metadata = useAppMetadata();
  const control = useDesktopControlState();
  const query = useAdminReadModelState();
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

  const appVersion = metadata.data?.version ?? "0.20.0";
  const data = readModel.data
    ? mapAdminDashboardData({
        summary: readModel.data.operatorSummary,
        runtimeStatus: readModel.data.runtimeStatus,
        providers: readModel.data.providers,
        recentRequests: readModel.data.recentRequests,
        usageSummary: readModel.data.usageSummary,
        usageDay: readModel.data.usageDay,
        adminBaseUrl: readModel.data.endpoint.adminBaseUrl,
        appVersion,
      })
    : mockDashboardData;
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
