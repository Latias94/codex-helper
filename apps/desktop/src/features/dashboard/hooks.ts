import { useQuery } from "@tanstack/react-query";

import { queryKeys } from "@/lib/api/query-keys";
import { getAppMetadata } from "@/lib/tauri/commands";

export function useAppMetadata() {
  return useQuery({
    queryFn: getAppMetadata,
    queryKey: queryKeys.appMetadata,
  });
}
