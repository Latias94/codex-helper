import { AlertTriangle, Loader2, Wifi } from "lucide-react";

import { Badge, Button } from "@/components/ui";
import type { DataSource } from "@/lib/api/types";

export function DataStateBanner({
  source,
  isLoading,
  isRefreshing,
  errorMessage,
  onRefresh,
}: {
  source: DataSource;
  isLoading: boolean;
  isRefreshing: boolean;
  errorMessage?: string;
  onRefresh?: () => void;
}) {
  if (source === "live" && !isRefreshing && !errorMessage) {
    return null;
  }

  return (
    <div className="mb-4 flex items-center justify-between rounded-2xl border border-amber-200 bg-amber-50/80 px-4 py-3 text-sm text-amber-800">
      <div className="flex min-w-0 items-center gap-3">
        {isLoading || isRefreshing ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : (
          <AlertTriangle className="h-4 w-4" />
        )}
        <div className="min-w-0">
          <div className="font-medium">
            {source === "live" ? "正在刷新本地 admin API 数据" : "当前展示离线示例数据"}
          </div>
          <div className="truncate text-xs text-amber-700">
            {errorMessage
              ? `无法连接本地 admin API：${errorMessage}`
              : "启动或附加 codex-helper 本地代理后会自动切换为实时数据。"}
          </div>
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <Badge variant={source === "live" ? "success" : "warning"}>
          <Wifi className="h-3.5 w-3.5" />
          {source === "live" ? "Live" : "Mock fallback"}
        </Badge>
        {onRefresh ? (
          <Button variant="outline" onClick={onRefresh}>
            重试刷新
          </Button>
        ) : null}
      </div>
    </div>
  );
}
