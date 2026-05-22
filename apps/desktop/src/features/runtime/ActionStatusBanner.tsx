import { AlertTriangle, CheckCircle2, Loader2 } from "lucide-react";

import type { RuntimeActionStatus } from "@/features/runtime/actions";
import { cn } from "@/lib/utils";

export function ActionStatusBanner({
  status,
  busy,
}: {
  status: RuntimeActionStatus;
  busy?: boolean;
}) {
  if (!busy && status.kind === "idle") {
    return null;
  }

  const tone = busy ? "info" : status.kind;
  return (
    <div
      className={cn(
        "flex items-start gap-2 rounded-2xl border px-3 py-2 text-sm",
        tone === "info" && "border-teal-200 bg-teal-50 text-teal-700",
        tone === "success" && "border-emerald-200 bg-emerald-50 text-emerald-700",
        tone === "error" && "border-red-200 bg-red-50 text-red-700",
      )}
      role={tone === "error" ? "alert" : "status"}
    >
      {busy ? (
        <Loader2 className="mt-0.5 h-4 w-4 animate-spin" />
      ) : status.kind === "error" ? (
        <AlertTriangle className="mt-0.5 h-4 w-4" />
      ) : (
        <CheckCircle2 className="mt-0.5 h-4 w-4" />
      )}
      <span>{busy ? "正在执行本地控制动作…" : status.message}</span>
    </div>
  );
}
