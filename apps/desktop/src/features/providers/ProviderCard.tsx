import { Activity, Ban, Edit3, RefreshCw, RotateCcw, Save, ShieldCheck, X, Zap } from "lucide-react";
import { type FormEvent, type ReactNode, useEffect, useState } from "react";

import { Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Input, Switch } from "@/components/ui";
import { providerCommonEditSchema } from "@/features/providers/schemas";
import type { ProviderCardView } from "@/lib/api/types";
import type { ProviderCommonEditPayload } from "@/lib/tauri/commands";

export function ProviderCard({
  provider,
  onProbe,
  onRefreshBalance,
  onSetActive,
  onDisable,
  onClearOverride,
  onSaveCommonEdit,
  busy,
}: {
  provider: ProviderCardView;
  onProbe?: () => void;
  onRefreshBalance?: () => void;
  onSetActive?: () => void;
  onDisable?: () => void;
  onClearOverride?: () => void;
  onSaveCommonEdit?: (payload: ProviderCommonEditPayload) => void;
  busy?: boolean;
}) {
  const healthy = provider.health === "Healthy";
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!provider.editable) {
      setEditing(false);
    }
  }, [provider.editable]);

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
          <Info label="Continuity" value={provider.continuityDomain || "endpoint"} mono />
          <Info label="Balance" value={provider.balance} />
          <Info label="Latency" value={provider.latency} />
        </div>
        <div className="flex flex-wrap gap-2">
          {provider.capabilities.map((capability) => (
            <Badge key={capability} variant="teal">
              {capability}
            </Badge>
          ))}
          {provider.endpointCount > 1 && <Badge variant="warning">raw TOML</Badge>}
        </div>
        {editing && (
          <ProviderEditForm
            provider={provider}
            busy={busy}
            onCancel={() => setEditing(false)}
          onSave={async (payload) => {
            await onSaveCommonEdit?.(payload);
            setEditing(false);
          }}
        />
        )}
        {!editing && provider.editBlockedReason && (
          <p className="rounded-xl border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-700">
            {provider.editBlockedReason}
          </p>
        )}
        <div className="flex items-center justify-between">
          <span className="text-xs text-slate-500">Last used {provider.lastUsed}</span>
          <div className="flex flex-wrap justify-end gap-2">
            <Button
              variant="outline"
              onClick={() => setEditing((value) => !value)}
              disabled={busy || !provider.editable}
              aria-label={`编辑 ${provider.name}`}
            >
              <Edit3 className="h-4 w-4" />
              编辑
            </Button>
            <Button variant={provider.active ? "secondary" : "default"} onClick={onSetActive} disabled={busy}>
              <Zap className="h-4 w-4" />
              Set Active
            </Button>
            <Button variant="outline" onClick={onProbe} disabled={busy}>
              <Activity className="h-4 w-4" />
              Probe
            </Button>
            <Button variant="outline" className="w-9 px-0" onClick={onRefreshBalance} disabled={busy}>
              <RefreshCw className="h-4 w-4" />
            </Button>
            <Button variant="warning" className="w-9 px-0" onClick={onDisable} disabled={busy}>
              <Ban className="h-4 w-4" />
            </Button>
            <Button variant="outline" className="w-9 px-0" onClick={onClearOverride} disabled={busy}>
              <RotateCcw className="h-4 w-4" />
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function ProviderEditForm({
  provider,
  busy,
  onCancel,
  onSave,
}: {
  provider: ProviderCardView;
  busy?: boolean;
  onCancel: () => void;
  onSave: (payload: ProviderCommonEditPayload) => Promise<void> | void;
}) {
  const [alias, setAlias] = useState(provider.alias ?? provider.name);
  const [baseUrl, setBaseUrl] = useState(provider.baseUrl);
  const [continuityDomain, setContinuityDomain] = useState(provider.continuityDomain ?? "");
  const [enabled, setEnabled] = useState(provider.enabled);
  const [authTokenEnv, setAuthTokenEnv] = useState("");
  const [apiKeyEnv, setApiKeyEnv] = useState("");
  const [error, setError] = useState<string | null>(null);

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const parsed = providerCommonEditSchema.safeParse({
      service: "codex",
      providerName: provider.id ?? provider.name,
      alias: alias.trim(),
      baseUrl: baseUrl.trim(),
      continuityDomain: continuityDomain.trim(),
      enabled,
      ...(authTokenEnv.trim() ? { authTokenEnv: authTokenEnv.trim() } : {}),
      ...(apiKeyEnv.trim() ? { apiKeyEnv: apiKeyEnv.trim() } : {}),
    });
    if (!parsed.success) {
      setError(parsed.error.issues[0]?.message ?? "Provider 表单校验失败。");
      return;
    }
    setError(null);
    void Promise.resolve(onSave(parsed.data)).catch(() => {
      // The shared action banner reports command failures; keep the form open for correction.
    });
  }

  return (
    <form
      className="space-y-3 rounded-2xl border border-teal-200 bg-teal-50/60 p-3"
      onSubmit={handleSubmit}
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Alias">
          <Input
            aria-label={`Alias for ${provider.name}`}
            id={`provider-alias-${provider.id ?? provider.name}`}
            value={alias}
            onChange={(event) => setAlias(event.target.value)}
            placeholder={provider.id ?? provider.name}
          />
        </Field>
        <Field label="Base URL">
          <Input
            aria-label={`Base URL for ${provider.name}`}
            id={`provider-base-url-${provider.id ?? provider.name}`}
            value={baseUrl}
            onChange={(event) => setBaseUrl(event.target.value)}
            placeholder="https://api.example.com/v1"
          />
        </Field>
        <Field label="Auth token env">
          <Input
            aria-label={`Auth token env for ${provider.name}`}
            id={`provider-auth-token-env-${provider.id ?? provider.name}`}
            value={authTokenEnv}
            onChange={(event) => setAuthTokenEnv(event.target.value)}
            placeholder="PROVIDER_API_KEY"
          />
        </Field>
        <Field label="Continuity domain">
          <Input
            aria-label={`Continuity domain for ${provider.name}`}
            id={`provider-continuity-domain-${provider.id ?? provider.name}`}
            value={continuityDomain}
            onChange={(event) => setContinuityDomain(event.target.value)}
            placeholder="relay-cluster-a"
          />
        </Field>
        <Field label="API key env">
          <Input
            aria-label={`API key env for ${provider.name}`}
            id={`provider-api-key-env-${provider.id ?? provider.name}`}
            value={apiKeyEnv}
            onChange={(event) => setApiKeyEnv(event.target.value)}
            placeholder="OPENAI_API_KEY"
          />
        </Field>
      </div>
      <label className="flex items-center justify-between rounded-xl border border-teal-100 bg-white/70 px-3 py-2 text-sm text-slate-700">
        <span>
          启用 provider
          <span className="ml-2 text-xs text-slate-400">写入 configured enabled</span>
        </span>
        <Switch
          checked={enabled}
          onCheckedChange={setEnabled}
          aria-label={`启用 ${provider.name}`}
        />
      </label>
      {error && <p className="text-xs text-red-600">{error}</p>}
      <p className="text-xs leading-5 text-slate-500">
        常用表单只修改 alias、base_url、continuity_domain、enabled 和 env 名称；env 输入留空会保留现有设置。
        tags、limits、model mapping 等高级字段会留在 config.toml 中。多 endpoint provider 继续使用 raw TOML。
      </p>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onCancel} disabled={busy}>
          <X className="h-4 w-4" />
          取消
        </Button>
        <Button type="submit" disabled={busy}>
          <Save className="h-4 w-4" />
          保存
        </Button>
      </div>
    </form>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="space-y-1 text-xs font-medium text-slate-500">
      <span>{label}</span>
      {children}
    </label>
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
