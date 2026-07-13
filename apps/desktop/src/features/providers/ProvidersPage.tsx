import { Database } from "lucide-react";

import { PageHeader } from "@/app/AppShell";
import { DataStateBanner } from "@/components/page/DataStateBanner";
import { EmptyState } from "@/components/page/EmptyState";
import { Badge } from "@/components/ui";
import { ProviderCard } from "@/features/providers/ProviderCard";
import { useProvidersData } from "@/features/providers/hooks";

export function ProvidersPage() {
  const providersState = useProvidersData();
  const { providers } = providersState.data;
  const activeControlEvents = providers.reduce((total, provider) => total + provider.controlBadges.length, 0);

  return (
    <div className="flex min-h-[calc(100vh-5rem)] flex-col">
      <PageHeader
        title="供应商"
        subtitle="查看 canonical operator read model 中的 provider 和 endpoint inventory"
      />
      <DataStateBanner state={providersState.state} onRefresh={providersState.refetch} />

      <div className="mb-4 flex shrink-0 flex-wrap items-center gap-2 border-y border-slate-200 bg-white/70 px-1 py-3">
        <Badge variant="muted">{providers.length} providers</Badge>
        <Badge variant={activeControlEvents > 0 ? "warning" : "muted"}>
          control {activeControlEvents}
        </Badge>
      </div>

      <div className="app-scroll grid min-h-0 flex-1 grid-cols-2 content-start gap-4 overflow-y-auto pr-1">
          {providers.length === 0 ? (
            <div className="col-span-2">
              <EmptyState
                icon={Database}
                title="还没有可显示的供应商"
                description="连接到本地 admin API 后，这里会显示 coherent operator read model 中的供应商投影。"
              />
            </div>
          ) : (
            providers.map((provider) => <ProviderCard key={provider.name} provider={provider} />)
          )}
      </div>
    </div>
  );
}
