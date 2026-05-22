import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useState } from "react";

import { queryKeys } from "@/lib/api/query-keys";
import type { DesktopActionResult } from "@/lib/api/types";
import {
  applyProviderRuntimeOverride,
  applySessionOverrides,
  attachExistingProxy,
  getDesktopControlState,
  probeStation,
  refreshProviderBalances,
  reloadRuntime,
  resetSessionOverrides,
  setGlobalRouteOverride,
  startDesktopProxy,
  stopProxy,
  switchCodex,
  type CodexPreset,
  type ProviderRuntimeState,
  type SessionOverrideDimension,
} from "@/lib/tauri/commands";

export const CONTROL_CONFIRMATIONS = {
  stopOwned: "STOP OWNED PROXY",
  stopAttached: "STOP ATTACHED PROXY",
  switchCodexOn: "SWITCH CODEX",
  switchCodexOff: "SWITCH OFF CODEX",
} as const;

export type RuntimeActionStatus = {
  kind: "idle" | "success" | "error";
  message: string;
  action?: string;
};

export function useDesktopControlState() {
  return useQuery({
    queryFn: getDesktopControlState,
    queryKey: queryKeys.admin.controlState,
    retry: false,
    staleTime: 5_000,
  });
}

export function useRuntimeActions() {
  const queryClient = useQueryClient();
  const [status, setStatus] = useState<RuntimeActionStatus>({ kind: "idle", message: "" });

  const invalidateRuntime = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.admin.controlState }),
      queryClient.invalidateQueries({ queryKey: queryKeys.admin.readModel }),
    ]);
  }, [queryClient]);

  const mutationOptions = <TInput,>(
    action: (input: TInput) => Promise<DesktopActionResult>,
  ) => ({
    mutationFn: action,
    onError: (error: unknown) => {
      setStatus({ kind: "error", message: errorMessage(error) });
    },
    onSuccess: async (result: DesktopActionResult) => {
      setStatus({ kind: "success", action: result.action, message: result.message });
      await invalidateRuntime();
    },
  });

  const startProxy = useMutation(mutationOptions<void>(() => startDesktopProxy()));
  const attachProxy = useMutation(mutationOptions<void>(() => attachExistingProxy()));
  const reload = useMutation(mutationOptions<void>(() => reloadRuntime()));
  const stopOwned = useMutation(
    mutationOptions<string>((confirmation) => stopProxy({ scope: "owned", confirmation })),
  );
  const stopAttached = useMutation(
    mutationOptions<string>((confirmation) => stopProxy({ scope: "attached", confirmation })),
  );
  const switchOn = useMutation(
    mutationOptions<{
      preset?: CodexPreset;
      responsesWebsocket?: boolean;
      confirmation: string;
    }>((payload) =>
      switchCodex({
        enabled: true,
        preset: payload.preset,
        responsesWebsocket: payload.responsesWebsocket,
        confirmation: payload.confirmation,
      }),
    ),
  );
  const switchOff = useMutation(
    mutationOptions<string>((confirmation) => switchCodex({ enabled: false, confirmation })),
  );
  const probe = useMutation(mutationOptions<{ stationName: string }>(probeStation));
  const refreshBalances = useMutation(
    mutationOptions<{ stationName?: string; providerId?: string }>(refreshProviderBalances),
  );
  const setProviderOverride = useMutation(
    mutationOptions<{
      providerName: string;
      endpointName?: string;
      enabled?: boolean;
      clearEnabled?: boolean;
      runtimeState?: ProviderRuntimeState;
      clearRuntimeState?: boolean;
    }>(applyProviderRuntimeOverride),
  );
  const setGlobalRoute = useMutation(
    mutationOptions<{ target?: string | null }>(setGlobalRouteOverride),
  );
  const setSessionOverrides = useMutation(
    mutationOptions<{
      sessionId: string;
      model?: string;
      reasoningEffort?: string;
      stationName?: string;
      routeTarget?: string;
      serviceTier?: string;
      clear?: SessionOverrideDimension[];
    }>(applySessionOverrides),
  );
  const resetSession = useMutation(mutationOptions<{ sessionId: string }>(resetSessionOverrides));

  const isBusy = [
    startProxy,
    attachProxy,
    reload,
    stopOwned,
    stopAttached,
    switchOn,
    switchOff,
    probe,
    refreshBalances,
    setProviderOverride,
    setGlobalRoute,
    setSessionOverrides,
    resetSession,
  ].some((mutation) => mutation.isPending);

  return {
    status,
    isBusy,
    startProxy,
    attachProxy,
    reload,
    stopOwned,
    stopAttached,
    switchOn,
    switchOff,
    probe,
    refreshBalances,
    setProviderOverride,
    setGlobalRoute,
    setSessionOverrides,
    resetSession,
    resetStatus: () => setStatus({ kind: "idle", message: "" }),
  };
}

function errorMessage(error: unknown) {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "object" && error !== null) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") {
      return message;
    }
  }
  return String(error);
}
