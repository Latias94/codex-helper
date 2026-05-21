import { Clock3, Database, Link2, RefreshCw, Server } from "lucide-react";

import { Button, Card } from "@/components/ui";
import { mockRuntime } from "@/lib/api/mock-data";
import type { RuntimeSummary } from "@/lib/api/types";

export function StatusStrip({
  runtime = mockRuntime,
  onRefresh,
}: {
  runtime?: RuntimeSummary;
  onRefresh?: () => void;
}) {
  return (
    <Card className="mb-4 flex items-center justify-between px-5 py-3">
      <div className="flex items-center gap-8 text-sm">
        <span className="flex items-center gap-2 text-slate-700">
          <span className="h-2.5 w-2.5 rounded-full bg-emerald-500" />
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
      </div>
      <Button variant="outline" onClick={onRefresh}>
        <RefreshCw className="h-4 w-4" />
        刷新状态
      </Button>
    </Card>
  );
}
