import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useState } from "react";

import { queryKeys } from "@/lib/api/query-keys";
import type { DesktopActionResult } from "@/lib/api/types";
import {
  attachExistingProxy,
  getDesktopControlState,
  startDesktopProxy,
  switchCodex,
} from "@/lib/tauri/commands";

export const CONTROL_CONFIRMATIONS = {
  switchCodexOn: "SWITCH CODEX",
  switchCodexOff: "SWITCH OFF CODEX",
} as const;

export type RuntimeActionStatus = {
  kind: "idle" | "success" | "error";
  message: string;
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
    onError: async (error: unknown) => {
      setStatus({ kind: "error", message: errorMessage(error) });
      await invalidateRuntime();
    },
    onSuccess: async (result: DesktopActionResult) => {
      setStatus({ kind: "success", message: result.message });
      await invalidateRuntime();
    },
  });

  const startProxy = useMutation(mutationOptions<void>(() => startDesktopProxy()));
  const attachProxy = useMutation(mutationOptions<void>(() => attachExistingProxy()));
  const switchOn = useMutation(
    mutationOptions<string>((confirmation) =>
      switchCodex({
        enabled: true,
        confirmation,
      }),
    ),
  );
  const switchOff = useMutation(
    mutationOptions<string>((confirmation) => switchCodex({ enabled: false, confirmation })),
  );
  const isBusy = [
    startProxy,
    attachProxy,
    switchOn,
    switchOff,
  ].some((mutation) => mutation.isPending);

  return {
    status,
    isBusy,
    startProxy,
    attachProxy,
    switchOn,
    switchOff,
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
