import type { LucideIcon } from "lucide-react";

import { Badge, Card } from "@/components/ui";

export function MetricCard({
  label,
  value,
  note,
  icon: Icon,
  tone,
}: {
  label: string;
  value: string;
  note: string;
  icon: LucideIcon;
  tone: "success" | "warning" | "teal" | "blue" | "default";
}) {
  const variant = tone === "default" ? "muted" : tone;
  return (
    <Card className="p-4">
      <div className="flex items-start justify-between">
        <div className="text-sm text-slate-500">{label}</div>
        <Badge variant={variant} className="px-2">
          <Icon className="h-3.5 w-3.5" />
        </Badge>
      </div>
      <div className="mt-3 text-2xl font-semibold tracking-tight text-slate-950">{value}</div>
      <div className="mt-1 truncate text-xs text-slate-500">{note}</div>
    </Card>
  );
}
