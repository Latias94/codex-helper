import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "@/lib/api/query-keys";
import { getKnownPaths, getLaunchAtLoginEnabled, setLaunchAtLoginEnabled } from "@/lib/tauri/commands";

export function useKnownPaths() {
  return useQuery({
    queryFn: getKnownPaths,
    queryKey: queryKeys.knownPaths,
    staleTime: 60_000,
  });
}

export function useLaunchAtLogin() {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryFn: getLaunchAtLoginEnabled,
    queryKey: queryKeys.launchAtLogin,
    retry: false,
    staleTime: 30_000,
  });
  const mutation = useMutation({
    mutationFn: setLaunchAtLoginEnabled,
    onSuccess: async (enabled) => {
      queryClient.setQueryData(queryKeys.launchAtLogin, enabled);
      await queryClient.invalidateQueries({ queryKey: queryKeys.launchAtLogin });
    },
  });

  return {
    ...query,
    setEnabled: mutation.mutateAsync,
    isSaving: mutation.isPending,
    saveError: mutation.error,
  };
}
