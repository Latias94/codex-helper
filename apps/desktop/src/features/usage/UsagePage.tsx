import { BadgeDollarSign, Clock3, DatabaseZap, FileDown, Zap } from "lucide-react";

import { PageHeader } from "@/app/AppShell";
import { DataStateBanner } from "@/components/page/DataStateBanner";
import { MetricCard } from "@/components/page/MetricCard";
import { UsageTable } from "@/features/usage/UsageTable";
import { useUsageData } from "@/features/usage/hooks";

export function UsagePage() {
  const usage = useUsageData();
  const { rows, summary } = usage.data;

  return (
    <div className="flex min-h-[calc(100vh-5rem)] flex-col">
      <PageHeader title="用量" subtitle="查看本地请求历史、tokens、首 token 延迟和预估费用" />
      <DataStateBanner
        source={usage.source}
        isLoading={usage.isLoading}
        isRefreshing={usage.isRefreshing}
        errorMessage={usage.errorMessage}
        onRefresh={usage.refetch}
      />

      <div className="mb-4 grid shrink-0 grid-cols-4 gap-4">
        <MetricCard label="Total Requests" value={summary.totalRequests} note={`显示 1 至 ${rows.length}，共 ${summary.totalRows} 条`} icon={DatabaseZap} tone="blue" />
        <MetricCard label="Total Tokens" value={summary.totalTokens} note="input / output / cache grouped" icon={Zap} tone="teal" />
        <MetricCard label="预估费用" value={summary.estimatedCost} note="实际费用以供应商结算为准" icon={BadgeDollarSign} tone="warning" />
        <MetricCard label="Average Duration" value={summary.averageDuration} note={`First token ${summary.averageFirstToken}`} icon={Clock3} tone="default" />
      </div>

      <UsageTable rows={rows} totalRows={summary.totalRows} onRefresh={usage.refetch} />

      <div className="mt-4 flex items-center gap-2 text-sm text-slate-500">
        <FileDown className="h-4 w-4" />
        成本展示为预估值；行内 tooltip 展示 input、output、cache read 和 multiplier 明细。
      </div>
    </div>
  );
}
