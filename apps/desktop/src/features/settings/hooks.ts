import { useQuery } from "@tanstack/react-query";

import { queryKeys } from "@/lib/api/query-keys";
import { getKnownPaths } from "@/lib/tauri/commands";

export function useKnownPaths() {
  return useQuery({
    queryFn: getKnownPaths,
    queryKey: queryKeys.knownPaths,
    staleTime: 60_000,
  });
}
