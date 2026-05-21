import { ArrowDown, Plus, Search, SlidersHorizontal } from "lucide-react";

import { PageHeader } from "@/components/AppShell";
import { ProviderCard } from "@/components/ProviderCard";
import { Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Input, SelectBox } from "@/components/ui";
import { providers } from "@/data/mock";

export function ProvidersPage() {
  return (
    <>
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

      <div className="mb-4 flex items-center justify-between rounded-2xl border border-slate-200 bg-white/88 p-4 shadow-sm">
        <div className="flex items-center gap-3">
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

      <div className="grid grid-cols-[1fr_320px] gap-4">
        <div className="grid grid-cols-2 gap-4">
          {providers.map((provider) => (
            <ProviderCard key={provider.name} provider={provider} />
          ))}
        </div>

        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle>Default Route</CardTitle>
              <CardDescription>Codex 请求默认按此顺序尝试 provider。</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              {providers.map((provider, index) => (
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
    </>
  );
}
