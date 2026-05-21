import { Activity, CheckCircle2, Edit3, RefreshCw, ShieldCheck, Zap } from "lucide-react";

import { Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui";

export function ProviderCard({
  provider,
}: {
  provider: {
    name: string;
    host: string;
    auth: string;
    balance: string;
    health: string;
    latency: string;
    capabilities: string[];
    usage: string;
    lastUsed: string;
    active: boolean;
  };
}) {
  const healthy = provider.health === "Healthy";
  return (
    <Card className={provider.active ? "ring-2 ring-teal-500/20" : ""}>
      <CardHeader>
        <div className="flex items-start justify-between">
          <div>
            <CardTitle className="flex items-center gap-2">
              {provider.name}
              {provider.active && <Badge variant="teal">Active</Badge>}
            </CardTitle>
            <CardDescription className="font-mono">{provider.host}</CardDescription>
          </div>
          <Badge variant={healthy ? "success" : "warning"}>
            <ShieldCheck className="h-3.5 w-3.5" />
            {healthy ? "Healthy" : "Warning"}
          </Badge>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="grid grid-cols-2 gap-3 text-sm">
          <Info label="Auth" value={provider.auth} mono />
          <Info label="Balance" value={provider.balance} />
          <Info label="Latency" value={provider.latency} />
          <Info label="Usage today" value={provider.usage} />
        </div>
        <div className="flex flex-wrap gap-2">
          {provider.capabilities.map((capability) => (
            <Badge key={capability} variant="teal">
              {capability}
            </Badge>
          ))}
        </div>
        <div className="flex items-center justify-between">
          <span className="text-xs text-slate-500">Last used {provider.lastUsed}</span>
          <div className="flex gap-2">
            <Button variant={provider.active ? "secondary" : "default"}>
              <Zap className="h-4 w-4" />
              Set Active
            </Button>
            <Button variant="outline">
              <Activity className="h-4 w-4" />
              Probe
            </Button>
            <Button variant="outline" className="w-9 px-0">
              <RefreshCw className="h-4 w-4" />
            </Button>
            <Button variant="outline" className="w-9 px-0">
              <Edit3 className="h-4 w-4" />
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function Info({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div>
      <div className="text-xs text-slate-400">{label}</div>
      <div className={mono ? "mt-1 truncate font-mono text-slate-700" : "mt-1 truncate text-slate-700"}>{value}</div>
    </div>
  );
}
