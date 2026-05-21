import type { LucideIcon } from "lucide-react";

import { Card } from "@/components/ui";

export function EmptyState({
  icon: Icon,
  title,
  description,
}: {
  icon: LucideIcon;
  title: string;
  description: string;
}) {
  return (
    <Card className="flex min-h-40 flex-col items-center justify-center p-6 text-center">
      <div className="mb-3 flex h-11 w-11 items-center justify-center rounded-2xl bg-slate-100 text-slate-500">
        <Icon className="h-5 w-5" />
      </div>
      <div className="font-medium text-slate-900">{title}</div>
      <p className="mt-1 max-w-sm text-sm leading-6 text-slate-500">{description}</p>
    </Card>
  );
}
