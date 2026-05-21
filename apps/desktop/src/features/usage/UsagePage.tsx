import { BadgeDollarSign, Clock3, DatabaseZap, FileDown } from "lucide-react";

import { PageHeader } from "@/app/AppShell";
import { MetricCard } from "@/components/page/MetricCard";
import { UsageTable } from "@/features/usage/UsageTable";
import { metrics } from "@/mocks/dashboard";

export function UsagePage() {
  return (
    <div className="flex min-h-[calc(100vh-5rem)] flex-col">
      <PageHeader title="用量" subtitle="查看本地请求历史、tokens、首 token 延迟和预估费用" />

      <div className="mb-4 grid shrink-0 grid-cols-4 gap-4">
        <MetricCard label="Total Requests" value="128" note="显示 1 至 20，共 128 条" icon={DatabaseZap} tone="blue" />
        <MetricCard label="Total Tokens" value="1.84M" note="input / output / cache grouped" icon={metrics[4].icon} tone="teal" />
        <MetricCard label="预估费用" value="$0.42" note="实际费用以供应商结算为准" icon={BadgeDollarSign} tone="warning" />
        <MetricCard label="Average Duration" value="2.4s" note="First token 780ms" icon={Clock3} tone="default" />
      </div>

      <UsageTable />

      <div className="mt-4 flex items-center gap-2 text-sm text-slate-500">
        <FileDown className="h-4 w-4" />
        成本展示为预估值；行内 tooltip 展示 input、output、cache read 和 multiplier 明细。
      </div>
    </div>
  );
}
