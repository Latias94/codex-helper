import { Clock3, Database, Link2, RefreshCw, Server } from "lucide-react";

import { Button, Card } from "@/components/ui";
import { mockRuntime } from "@/lib/api/mock-data";
import type { RuntimeSummary } from "@/lib/api/types";
import { cn } from "@/lib/utils";

export function StatusStrip({
  runtime = mockRuntime,
  onRefresh,
  healthy = true,
}: {
  runtime?: RuntimeSummary;
  onRefresh?: () => void;
  healthy?: boolean;
}) {
  return (
    <Card className="mb-4 flex items-center justify-between px-5 py-3">
      <div className="flex items-center gap-8 text-sm">
        <span className="flex items-center gap-2 text-slate-700">
          <span
            className={cn("h-2.5 w-2.5 rounded-full", healthy ? "bg-emerald-500" : "bg-amber-500")}
          />
          <Server className="h-4 w-4 text-slate-400" />
          本地代理 <b className="text-teal-700">{runtime.proxy} · {runtime.port}</b>
        </span>
        <span className="flex items-center gap-2 text-slate-700">
          <Link2 className="h-4 w-4 text-slate-400" />
          Codex <b className="text-emerald-700">{runtime.codex}</b>
        </span>
        <span className="flex items-center gap-2 text-slate-700">
          <Database className="h-4 w-4 text-slate-400" />
          当前供应商 <b className="text-teal-700">{runtime.provider}</b>
        </span>
        <span className="flex items-center gap-2 text-slate-500">
          <Clock3 className="h-4 w-4" />
          最近刷新 {runtime.updatedAtLabel}
        </span>
        <span
          className={cn(
            "rounded-full border px-2.5 py-1 text-xs font-medium",
            runtime.ownerMode === "desktop-owned" && "border-teal-200 bg-teal-50 text-teal-700",
            runtime.ownerMode === "attached" && "border-sky-200 bg-sky-50 text-sky-700",
            runtime.ownerMode === "unknown" && "border-amber-200 bg-amber-50 text-amber-700",
          )}
        >
          {runtime.ownerMode === "desktop-owned"
            ? "桌面托管"
            : runtime.ownerMode === "attached"
              ? "附加模式"
              : "生命周期 owner 待确认"}
        </span>
      </div>
      <Button variant="outline" onClick={onRefresh}>
        <RefreshCw className="h-4 w-4" />
        刷新状态
      </Button>
    </Card>
  );
}
