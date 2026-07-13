import { AlertTriangle, BadgeDollarSign, Clock3, DatabaseZap, FileDown, ShieldAlert, Zap } from "lucide-react";

import { PageHeader } from "@/app/AppShell";
import { DataStateBanner } from "@/components/page/DataStateBanner";
import { MetricCard } from "@/components/page/MetricCard";
import { Badge, Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui";
import { UsageTable } from "@/features/usage/UsageTable";
import { useUsageData } from "@/features/usage/hooks";
import type { UsageDimensionRowView } from "@/lib/api/types";

export function UsagePage() {
  const usage = useUsageData();
  const { coverage, hourly, modelRows, projectRows, providerRows, retryGate, rows, summary } = usage.data;

  return (
    <div className="flex min-h-[calc(100vh-5rem)] flex-col">
      <PageHeader title="用量" subtitle="查看今天的 tokens、费用、缓存命中、retry gate 和最近请求 drilldown" />
      <DataStateBanner
        state={usage.state}
        onRefresh={usage.refetch}
      />

      <div className="mb-4 grid shrink-0 grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
        <MetricCard label="今日请求" value={summary.totalRequests} note={`drilldown ${rows.length} 条 · ${summary.dayLabel}`} icon={DatabaseZap} tone="blue" />
        <MetricCard label="今日 Tokens" value={summary.totalTokens} note={`cache ${summary.cacheRate}`} icon={Zap} tone="teal" />
        <MetricCard label="预估费用" value={summary.estimatedCost} note={`错误率 ${summary.errorRate}`} icon={BadgeDollarSign} tone="warning" />
        <MetricCard label="平均响应" value={summary.averageDuration} note={`First token ${summary.averageFirstToken}`} icon={Clock3} tone="default" />
      </div>

      <div className="mb-4 grid shrink-0 grid-cols-1 gap-4 xl:grid-cols-[1.35fr_0.65fr]">
        <Card>
          <CardHeader>
            <div className="flex items-start justify-between gap-3">
              <div>
                <CardTitle>24 小时活动</CardTitle>
                <CardDescription>按本地日窗口聚合，recent request 不参与日总量计算。</CardDescription>
              </div>
              <Badge variant={coverage.isPartial ? "warning" : "success"}>
                {coverage.isPartial ? "Partial" : "Complete"}
              </Badge>
            </div>
          </CardHeader>
          <CardContent>
            <div className="flex h-44 items-end gap-1.5 rounded-xl bg-slate-50 p-3">
              {hourly.map((hour) => (
                <div key={hour.hour} className="flex min-w-0 flex-1 flex-col items-center gap-2">
                  <div
                    className="w-full rounded-t-md bg-teal-500/80"
                    title={`${hour.label}: ${hour.requests} requests, ${hour.totalTokens.toLocaleString()} tokens`}
                    style={{ height: `${hour.height}%` }}
                  />
                  <span className="hidden text-[10px] text-slate-400 2xl:inline">{hour.hour % 3 === 0 ? hour.hour : ""}</span>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>覆盖与 Retry Gate</CardTitle>
            <CardDescription>本地日窗口与当前控制动作。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3 text-sm">
            <div className="rounded-xl border border-slate-100 bg-slate-50 p-3">
              <div className="flex items-center gap-2 font-medium text-slate-800">
                <AlertTriangle className="h-4 w-4 text-amber-600" />
                覆盖状态
              </div>
              <div className="mt-1 text-slate-500">
                {coverage.isPartial
                  ? coverage.reason ?? "可用数据晚于本地日窗口起点，今日统计可能不完整。"
                  : `已加载 ${coverage.loadedRequests.toLocaleString()} 条请求。`}
              </div>
              <div className="mt-2 text-xs text-slate-400">
                source {coverage.source} · loaded {coverage.loadedRequests.toLocaleString()}
              </div>
            </div>
            <div className="rounded-xl border border-slate-100 bg-slate-50 p-3">
              <div className="flex items-center gap-2 font-medium text-slate-800">
                <ShieldAlert className="h-4 w-4 text-teal-700" />
                Retry Gate
              </div>
              <div className="mt-1 text-slate-500">
                {retryGate.active} active · {retryGate.activeCooldowns} cooldown · max {retryGate.maxRemaining}
              </div>
              {retryGate.reasons.length > 0 ? (
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {retryGate.reasons.map((reason) => (
                    <Badge key={reason.reason} variant="warning">
                      {reason.reason} {reason.active}
                    </Badge>
                  ))}
                </div>
              ) : null}
            </div>
          </CardContent>
        </Card>
      </div>

      <div className="mb-4 grid shrink-0 grid-cols-1 gap-4 xl:grid-cols-3">
        <DimensionPanel title="Provider" rows={providerRows} />
        <DimensionPanel title="Model" rows={modelRows} />
        <DimensionPanel title="Project" rows={projectRows} />
      </div>

      <UsageTable rows={rows} totalRows={summary.totalRows} onRefresh={usage.refetch} />

      <div className="mt-4 flex items-center gap-2 text-sm text-slate-500">
        <FileDown className="h-4 w-4" />
        成本展示为预估值；行内 tooltip 展示 input、output、cache read 和 multiplier 明细。
      </div>
    </div>
  );
}

function DimensionPanel({ title, rows }: { title: string; rows: UsageDimensionRowView[] }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>按今日 tokens 和费用聚合的前 8 项。</CardDescription>
      </CardHeader>
      <CardContent className="space-y-2">
        {rows.length === 0 ? (
          <div className="rounded-xl bg-slate-50 px-3 py-6 text-center text-sm text-slate-500">暂无今日数据</div>
        ) : rows.map((row) => (
          <div key={row.name} className="flex items-center justify-between gap-3 rounded-xl bg-slate-50 px-3 py-2">
            <div className="min-w-0">
              <div className="truncate font-medium text-slate-800">{row.name}</div>
              <div className="text-xs text-slate-500">
                {row.requests} req · {row.averageDuration} avg · err {row.errorRate}
              </div>
            </div>
            <div className="shrink-0 text-right text-sm">
              <div className="font-medium text-slate-900">{row.totalTokens}</div>
              <div className="text-xs text-slate-500">{row.cost}</div>
            </div>
          </div>
        ))}
      </CardContent>
    </Card>
  );
}
