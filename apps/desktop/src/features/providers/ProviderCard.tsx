import { Network } from "lucide-react";

import { Badge, Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui";
import type { ProviderCardView } from "@/lib/api/types";

export function ProviderCard({ provider }: { provider: ProviderCardView }) {
  const status = providerStatus(provider);

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <CardTitle>{provider.alias || provider.name}</CardTitle>
            <CardDescription className="truncate font-mono">
              {provider.alias ? provider.name : `${provider.endpointCount} endpoints`}
            </CardDescription>
          </div>
          <Badge variant={status.tone}>
            <Network className="h-3.5 w-3.5" />
            {status.label}
          </Badge>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="grid grid-cols-3 gap-3 text-sm">
          <Info label="Configured" value={yesNo(provider.configuredEnabled)} />
          <Info label="Effective" value={yesNo(provider.effectiveEnabled)} />
          <Info label="Routable" value={`${provider.routableEndpoints}/${provider.endpointCount}`} />
        </div>
        {provider.capacity ? <Info label="Provider capacity" value={provider.capacity} mono /> : null}

        <div className="space-y-2 border-t border-slate-100 pt-3">
          <div className="text-xs font-semibold uppercase text-slate-500">Endpoints</div>
          {provider.endpoints.length === 0 ? (
            <div className="text-sm text-slate-500">No configured endpoints</div>
          ) : (
            provider.endpoints.map((endpoint) => (
              <div key={endpoint.key} className="border-b border-slate-100 pb-2 last:border-b-0 last:pb-0">
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-medium text-slate-800">{endpoint.name}</div>
                    <div className="truncate font-mono text-xs text-slate-500">{endpoint.origin}</div>
                  </div>
                  <div className="flex shrink-0 flex-wrap justify-end gap-1">
                    <Badge variant="muted">priority {endpoint.priority}</Badge>
                    <Badge variant={endpoint.routable ? "success" : "muted"}>
                      {endpoint.routable ? "routable" : "not routable"}
                    </Badge>
                  </div>
                </div>
                <div className="mt-1 text-xs text-slate-500">
                  configured {yesNo(endpoint.configuredEnabled)} · effective {yesNo(endpoint.effectiveEnabled)} · state {endpoint.runtimeState}
                  {endpoint.capacity ? ` · capacity ${endpoint.capacity}` : ""}
                  {endpoint.policyActionCount > 0 ? ` · control ${endpoint.policyActionCount}` : ""}
                </div>
              </div>
            ))
          )}
        </div>

        {provider.controlBadges.length > 0 ? (
          <div className="rounded-md border border-amber-200 bg-amber-50/70 px-3 py-2">
            <div className="mb-2 flex items-center justify-between gap-3">
              <span className="text-xs font-semibold uppercase text-amber-700">Provider Control</span>
              <span className="text-xs text-amber-700">{provider.controlSummary}</span>
            </div>
            <div className="flex flex-wrap gap-2">
              {provider.controlBadges.map((badge) => (
                <Badge
                  key={badge.key}
                  variant={badge.tone === "warning" ? "warning" : badge.tone === "teal" ? "teal" : "muted"}
                  title={badge.detail}
                >
                  {badge.label}
                </Badge>
              ))}
            </div>
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

function Info({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <div className="text-xs text-slate-400">{label}</div>
      <div className={mono ? "mt-1 truncate font-mono text-slate-700" : "mt-1 truncate text-slate-700"}>
        {value}
      </div>
    </div>
  );
}

function providerStatus(provider: ProviderCardView) {
  if (!provider.configuredEnabled) {
    return { label: "disabled", tone: "muted" as const };
  }
  if (!provider.effectiveEnabled) {
    return { label: "ineffective", tone: "warning" as const };
  }
  if (provider.routableEndpoints > 0) {
    return { label: "routable", tone: "success" as const };
  }
  return { label: "not routable", tone: "warning" as const };
}

function yesNo(value: boolean) {
  return value ? "yes" : "no";
}
