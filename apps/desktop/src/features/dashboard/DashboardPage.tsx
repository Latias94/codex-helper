import { AlertTriangle, ArrowRight, CheckCircle2, Play, Power, RefreshCw, Settings2 } from "lucide-react";

import { PageHeader } from "@/app/AppShell";
import { MetricCard } from "@/components/page/MetricCard";
import { StatusStrip } from "@/components/shell/StatusStrip";
import { Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui";
import { chartBars, metrics, providers, recentRequests } from "@/mocks/dashboard";

export function DashboardPage() {
  return (
    <>
      <PageHeader title="仪表盘" subtitle="查看本地代理、Codex 连接、供应商健康和今日用量" />
      <StatusStrip />

      <div className="grid grid-cols-4 gap-4">
        {metrics.map((metric) => (
          <MetricCard key={metric.label} {...metric} />
        ))}
      </div>

      <div className="mt-4 grid grid-cols-[1.15fr_0.85fr] gap-4">
        <Card>
          <CardHeader>
            <CardTitle>工作状态</CardTitle>
            <CardDescription>确认 Codex 是否通过本地代理中转，并执行安全操作。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <WorkRow
              name="Codex"
              status="已连接"
              provider="CodeX Air"
              action="Switch Off"
              active
            />
            <WorkRow
              name="Claude Code"
              status="未启用"
              provider="未配置"
              action="Switch On"
            />
            <div className="flex flex-wrap gap-2 rounded-2xl bg-mint-50 p-3">
              <Button>
                <Play className="h-4 w-4" />
                Start Proxy
              </Button>
              <Button variant="outline">
                <Power className="h-4 w-4" />
                Switch On
              </Button>
              <Button variant="outline">
                <RefreshCw className="h-4 w-4" />
                Refresh
              </Button>
              <Button variant="outline">
                <Settings2 className="h-4 w-4" />
                Advanced connection settings
              </Button>
            </div>
            <div className="flex items-center gap-2 text-sm text-amber-700">
              <AlertTriangle className="h-4 w-4" />
              Attached mode 示例：关闭窗口只会 Detach，停止代理请到 Settings 使用 Stop Proxy。
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <div>
                <CardTitle>最近请求</CardTitle>
                <CardDescription>最新 5 条请求记录。</CardDescription>
              </div>
              <Button variant="ghost">
                View all usage
                <ArrowRight className="h-4 w-4" />
              </Button>
            </div>
          </CardHeader>
          <CardContent className="space-y-2">
            {recentRequests.map((request) => (
              <div key={`${request.model}-${request.time}`} className="flex items-center justify-between rounded-xl border border-slate-100 px-3 py-2">
                <div>
                  <div className="flex items-center gap-2">
                    <Badge variant={request.status === "ok" ? "success" : "warning"}>
                      {request.status}
                    </Badge>
                    <span className="font-medium text-slate-800">{request.model}</span>
                  </div>
                  <div className="mt-1 text-xs text-slate-500">
                    {request.provider} · {request.tokens}
                  </div>
                </div>
                <div className="text-right text-sm">
                  <div className="font-medium text-slate-900">{request.cost}</div>
                  <div className="text-xs text-slate-500">
                    {request.duration} · {request.time}
                  </div>
                </div>
              </div>
            ))}
          </CardContent>
        </Card>
      </div>

      <div className="mt-4 grid grid-cols-[0.9fr_1.1fr] gap-4">
        <Card>
          <CardHeader>
            <CardTitle>供应商健康</CardTitle>
            <CardDescription>余额、延迟和今日用量概览。</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            {providers.slice(0, 3).map((provider) => (
              <div key={provider.name} className="flex items-center justify-between rounded-xl bg-slate-50 px-3 py-3">
                <div>
                  <div className="font-medium text-slate-900">{provider.name}</div>
                  <div className="text-xs text-slate-500">{provider.balance} · {provider.latency}</div>
                </div>
                <Badge variant={provider.health === "Healthy" ? "success" : "warning"}>{provider.health}</Badge>
              </div>
            ))}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Token Usage Trend</CardTitle>
            <CardDescription>最近 12 个时间片的 tokens 使用趋势。</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex h-48 items-end gap-3 rounded-2xl bg-gradient-to-b from-mint-50 to-white p-4">
              {chartBars.map((height, index) => (
                <div key={index} className="flex flex-1 flex-col items-center gap-2">
                  <div
                    className="w-full rounded-t-xl bg-teal-500/75"
                    style={{ height: `${height}%` }}
                  />
                  <span className="text-[10px] text-slate-400">{index + 1}</span>
                </div>
              ))}
            </div>
          </CardContent>
        </Card>
      </div>
    </>
  );
}

function WorkRow({
  name,
  status,
  provider,
  action,
  active,
}: {
  name: string;
  status: string;
  provider: string;
  action: string;
  active?: boolean;
}) {
  return (
    <div className="flex items-center justify-between rounded-2xl border border-slate-100 p-4">
      <div className="flex items-center gap-3">
        <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-teal-50 text-teal-700">
          <CheckCircle2 className="h-5 w-5" />
        </div>
        <div>
          <div className="font-medium text-slate-900">{name}</div>
          <div className="text-sm text-slate-500">
            {status} · {provider}
          </div>
        </div>
      </div>
      <Button variant={active ? "warning" : "outline"}>{action}</Button>
    </div>
  );
}
