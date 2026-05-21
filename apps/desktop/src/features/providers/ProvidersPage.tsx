import { ArrowDown, Database, Plus, Search, SlidersHorizontal } from "lucide-react";

import { PageHeader } from "@/app/AppShell";
import { DataStateBanner } from "@/components/page/DataStateBanner";
import { EmptyState } from "@/components/page/EmptyState";
import { ProviderCard } from "@/features/providers/ProviderCard";
import { useProvidersData } from "@/features/providers/hooks";
import { Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Input, SelectBox } from "@/components/ui";

export function ProvidersPage() {
  const providersState = useProvidersData();
  const { providers, routeOrder } = providersState.data;

  return (
    <div className="flex min-h-[calc(100vh-5rem)] flex-col">
      <PageHeader
        title="供应商"
        subtitle="管理本地 relay providers、凭证来源、健康状态和默认路由顺序"
        action={
          <Button>
            <Plus className="h-4 w-4" />
            添加供应商
          </Button>
        }
      />
      <DataStateBanner
        source={providersState.source}
        isLoading={providersState.isLoading}
        isRefreshing={providersState.isRefreshing}
        errorMessage={providersState.errorMessage}
        onRefresh={providersState.refetch}
      />

      <div className="mb-4 flex shrink-0 items-center justify-between rounded-2xl border border-slate-200 bg-white/88 p-4 shadow-sm">
        <div className="flex min-w-0 flex-wrap items-center gap-3">
          <div className="relative">
            <Search className="absolute left-3 top-2.5 h-4 w-4 text-slate-400" />
            <Input className="w-80 pl-9" placeholder="搜索供应商、Host 或能力" />
          </div>
          <SelectBox defaultValue="all">
            <option value="all">全部状态</option>
            <option value="healthy">Healthy</option>
            <option value="warning">Warning</option>
          </SelectBox>
          <Badge variant="teal">responses</Badge>
          <Badge variant="teal">compact</Badge>
          <Badge variant="teal">imagegen</Badge>
        </div>
        <Button variant="outline">
          <SlidersHorizontal className="h-4 w-4" />
          凭证列表模式
        </Button>
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-[1fr_320px] gap-4">
        <div className="app-scroll grid min-h-0 grid-cols-2 content-start gap-4 overflow-y-auto pr-1">
          {providers.length === 0 ? (
            <div className="col-span-2">
              <EmptyState
                icon={Database}
                title="还没有可显示的供应商"
                description="连接到本地 admin API 后，这里会读取 /providers 与 /operator/summary 的供应商配置。"
              />
            </div>
          ) : providers.map((provider) => (
            <ProviderCard key={provider.name} provider={provider} />
          ))}
        </div>

        <div className="space-y-4">
          <Card className="sticky top-0">
            <CardHeader>
              <CardTitle>Default Route</CardTitle>
              <CardDescription>Codex 请求默认按此顺序尝试 provider。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              {routeOrder.map((provider, index) => (
                <div key={provider.name} className="flex items-center gap-3 rounded-xl border border-slate-100 bg-slate-50 p-3">
                  <Badge variant={index === 0 ? "teal" : "muted"}>#{index + 1}</Badge>
                  <div className="min-w-0 flex-1">
                    <div className="truncate font-medium text-slate-900">{provider.name}</div>
                    <div className="truncate text-xs text-slate-500">{provider.host}</div>
                  </div>
                  <ArrowDown className="h-4 w-4 text-slate-400" />
                </div>
              ))}
              <Button variant="outline" className="w-full">高级路由设置</Button>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>凭证提示</CardTitle>
              <CardDescription>API keys 是 provider 的 auth 字段，不单独作为顶级页面。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2 text-sm text-slate-600">
              <p>优先使用环境变量保存敏感值，界面只显示 masked key 或 env var 名称。</p>
              <p>Provider 详情里可以打开凭证列表、诊断日志和模型映射。</p>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
