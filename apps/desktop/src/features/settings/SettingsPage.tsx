import { useState } from "react";

import { AlertTriangle, ChevronDown, Copy, FolderOpen, RefreshCw } from "lucide-react";

import { PageHeader } from "@/app/AppShell";
import { DataStateBanner } from "@/components/page/DataStateBanner";
import { StatusStrip } from "@/components/shell/StatusStrip";
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Input,
  Segment,
  SelectBox,
  Switch,
} from "@/components/ui";
import { CONTROL_CONFIRMATIONS, useRuntimeActions } from "@/features/runtime/actions";
import { ActionStatusBanner } from "@/features/runtime/ActionStatusBanner";
import { useRuntimeSummary } from "@/features/runtime/hooks";
import { useKnownPaths } from "@/features/settings/hooks";
import { hideMainWindow, quitApp, type CodexPreset } from "@/lib/tauri/commands";

const advancedRows = [
  ["会话覆盖", "为单个会话选择 provider 或模型策略"],
  ["高级路由", "查看模型映射和默认路由"],
  ["Relay 诊断", "探测连接和能力"],
  ["请求 Trace", "查看详细请求链路"],
  ["原始配置", "只读预览配置文件"],
  ["日志与缓存", "打开日志、缓存目录"],
];

export function SettingsPage() {
  const knownPaths = useKnownPaths();
  const runtime = useRuntimeSummary();
  const actions = useRuntimeActions();
  const [codexPreset, setCodexPreset] = useState<CodexPreset>("chatgpt-bridge");
  const [globalRouteTarget, setGlobalRouteTarget] = useState("");
  const [sessionId, setSessionId] = useState("");
  const [sessionRouteTarget, setSessionRouteTarget] = useState("");
  const [stopPhrase, setStopPhrase] = useState("");
  const paths = knownPaths.data;
  const runDesktopCommand = (command: () => Promise<unknown>) => {
    void command().catch((error) => {
      console.warn("desktop app command failed", error);
    });
  };

  return (
    <>
      <PageHeader title="设置" subtitle="配置桌面行为、本地代理、Codex 连接和高级工具" />
      <DataStateBanner
        state={runtime.state}
        onRefresh={runtime.refetch}
      />
      <StatusStrip
        runtime={runtime.data}
        healthy={runtime.source === "live" && !runtime.state.isStale}
        onRefresh={runtime.refetch}
      />
      <div className="mb-4">
        <ActionStatusBanner status={actions.status} busy={actions.isBusy} />
      </div>

      <div className="grid grid-cols-2 gap-4">
        <SettingsCard title="桌面行为" description="控制应用启动、托盘和窗口关闭方式。">
          <ToggleRow label="开机启动" checked={false} />
          <ToggleRow label="启用托盘" checked />
          <FieldRow label="关闭窗口时">
            <Badge variant="teal">隐藏到托盘</Badge>
          </FieldRow>
          <ToggleRow label="启动时自动启动本地代理（待实现）" checked={false} />
        </SettingsCard>

        <SettingsCard title="外观与语言" description="调整界面语言和显示偏好。">
          <FieldRow label="默认语言">
            <SelectBox defaultValue="zh" className="w-48">
              <option value="zh">中文</option>
              <option value="en">English</option>
            </SelectBox>
          </FieldRow>
          <FieldRow label="主题">
            <Segment items={["跟随系统", "浅色", "深色"]} value="跟随系统" />
          </FieldRow>
          <FieldRow label="界面密度">
            <Segment items={["舒适", "紧凑"]} value="舒适" />
          </FieldRow>
        </SettingsCard>

        <SettingsCard title="本地代理" description="本机代理监听地址和运行时配置。">
          <div className="grid grid-cols-2 gap-3">
            <Field label="Host" value="127.0.0.1" />
            <Field label="Port" value={String(runtime.data.port)} />
          </div>
          <FieldRow label="Endpoint">
            <div className="flex items-center gap-2">
              <Input value={runtime.data.endpoint} readOnly className="w-64 font-mono" />
              <Button variant="outline" className="w-9 px-0"><Copy className="h-4 w-4" /></Button>
            </div>
          </FieldRow>
          <div className="flex gap-2">
            <Badge variant="teal">{runtime.source === "live" ? "已连接 admin API" : "等待本地运行时"}</Badge>
            <Badge variant={runtime.source === "live" ? "success" : "warning"}>
              Admin {runtime.data.adminPort}
            </Badge>
            <Badge variant={runtime.state.ownerMode === "desktop-owned" ? "teal" : runtime.state.ownerMode === "attached" ? "blue" : "warning"}>
              {runtime.state.ownerMode === "desktop-owned"
                ? "桌面托管"
                : runtime.state.ownerMode === "attached"
                  ? "附加模式"
                  : "Owner 待确认"}
            </Badge>
          </div>
          <p className="text-xs leading-5 text-amber-700">
            生命周期规则：关闭窗口只隐藏到托盘，退出桌面端不会停止代理；只有输入确认短语的 Stop Proxy 才会请求 runtime shutdown。
          </p>
          <div className="flex gap-2 pt-2">
            <Button variant="outline"><Copy className="h-4 w-4" />复制 Endpoint</Button>
            <Button
              variant="outline"
              disabled={!runtime.state.canUseLiveActions || actions.reload.isPending}
              onClick={() => actions.reload.mutate()}
            >
              <RefreshCw className="h-4 w-4" />
              重新加载运行时
            </Button>
            <Button variant="outline"><FolderOpen className="h-4 w-4" />打开日志目录</Button>
          </div>
        </SettingsCard>

        <SettingsCard title="Codex 连接" description="控制 Codex 是否通过本地代理中转。">
          <ToggleRow label="Codex 中转" checked />
          <FieldRow label="当前预设">
            <SelectBox
              value={codexPreset}
              onChange={(event) => setCodexPreset(event.currentTarget.value as CodexPreset)}
              className="w-56"
            >
              <option value="default">default</option>
              <option value="chatgpt-bridge">chatgpt-bridge</option>
              <option value="imagegen-bridge">imagegen-bridge</option>
              <option value="official-relay">official-relay</option>
              <option value="official-imagegen">official-imagegen</option>
            </SelectBox>
          </FieldRow>
          <FieldRow label="当前供应商">
            <SelectBox value={runtime.data.provider} className="w-56" disabled>
              <option value={runtime.data.provider}>{runtime.data.provider}</option>
            </SelectBox>
          </FieldRow>
          <ToggleRow label="Responses WebSocket" checked />
          <div className="flex flex-wrap gap-2">
            <Badge variant="teal">responses</Badge>
            <Badge variant="teal">compact</Badge>
            <Badge variant="teal">imagegen</Badge>
          </div>
          <div className="flex gap-2 pt-2">
            <Button variant="outline">运行诊断</Button>
            <Button
              variant="outline"
              disabled={!runtime.state.canUseLiveActions || actions.switchOn.isPending}
              onClick={() =>
                actions.switchOn.mutate({
                  confirmation: CONTROL_CONFIRMATIONS.switchCodexOn,
                  preset: codexPreset,
                })
              }
            >
              切换预设
            </Button>
            <Button
              variant="warning"
              disabled={actions.switchOff.isPending}
              onClick={() => actions.switchOff.mutate(CONTROL_CONFIRMATIONS.switchCodexOff)}
            >
              关闭中转
            </Button>
          </div>
        </SettingsCard>

        <SettingsCard title="高级工具" description="日常使用不需要打开这些选项。">
          <div className="rounded-2xl border border-teal-100 bg-teal-50/60 p-3">
            <div className="text-sm font-medium text-slate-800">全局路由覆盖</div>
            <div className="mt-2 flex gap-2">
              <Input
                value={globalRouteTarget}
                onChange={(event) => setGlobalRouteTarget(event.currentTarget.value)}
                placeholder="route target / provider name"
                className="flex-1"
              />
              <Button
                variant="outline"
                disabled={!runtime.state.canUseLiveActions || actions.setGlobalRoute.isPending}
                onClick={() => actions.setGlobalRoute.mutate({ target: globalRouteTarget })}
              >
                设置
              </Button>
              <Button
                variant="ghost"
                disabled={!runtime.state.canUseLiveActions || actions.setGlobalRoute.isPending}
                onClick={() => actions.setGlobalRoute.mutate({ target: null })}
              >
                清除
              </Button>
            </div>
          </div>
          <div className="rounded-2xl border border-slate-200 bg-slate-50/70 p-3">
            <div className="text-sm font-medium text-slate-800">会话路由覆盖</div>
            <div className="mt-2 grid grid-cols-[1fr_1fr_auto_auto] gap-2">
              <Input
                value={sessionId}
                onChange={(event) => setSessionId(event.currentTarget.value)}
                placeholder="session id"
              />
              <Input
                value={sessionRouteTarget}
                onChange={(event) => setSessionRouteTarget(event.currentTarget.value)}
                placeholder="route target"
              />
              <Button
                variant="outline"
                disabled={!runtime.state.canUseLiveActions || !sessionId || actions.setSessionOverrides.isPending}
                onClick={() =>
                  actions.setSessionOverrides.mutate({
                    sessionId,
                    routeTarget: sessionRouteTarget,
                  })
                }
              >
                设置
              </Button>
              <Button
                variant="ghost"
                disabled={!runtime.state.canUseLiveActions || !sessionId || actions.resetSession.isPending}
                onClick={() => actions.resetSession.mutate({ sessionId })}
              >
                重置
              </Button>
            </div>
          </div>
          <div className="overflow-hidden rounded-2xl border border-slate-200">
            {advancedRows.map(([title, description]) => (
              <div key={title} className="flex items-center justify-between border-b border-slate-100 px-3 py-2.5 last:border-b-0">
                <div className="flex items-center gap-3">
                  <ChevronDown className="h-4 w-4 text-slate-400" />
                  <div>
                    <div className="text-sm font-medium text-slate-800">{title}</div>
                    <div className="text-xs text-slate-500">{description}</div>
                  </div>
                </div>
                <Button variant="ghost" className="text-teal-700">打开</Button>
              </div>
            ))}
          </div>
        </SettingsCard>

        <SettingsCard title="关于与路径" description="版本、本机路径和更新信息。">
          <PathRow label="Version" value="v0.16.0" />
          <PathRow label="Config" value={paths?.config ?? "~/.codex-helper/config.toml"} />
          <PathRow label="Logs" value={paths?.logs ?? "~/.codex-helper/logs"} />
          <PathRow label="Cache" value={paths?.cache ?? "~/.codex-helper/cache"} />
          {knownPaths.isError ? (
            <div className="rounded-xl border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-700">
              暂时无法读取桌面路径，当前展示默认路径占位。
            </div>
          ) : null}
          <div className="flex gap-2 pt-2">
            <Button variant="outline"><FolderOpen className="h-4 w-4" />打开配置目录</Button>
            <Button variant="outline"><RefreshCw className="h-4 w-4" />检查更新</Button>
          </div>
        </SettingsCard>

        <Card className="col-span-2 border-red-200 bg-red-50/40">
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-red-700">
              <AlertTriangle className="h-5 w-5" />
              危险操作
            </CardTitle>
            <CardDescription>
              退出应用、Detach 和 Stop Proxy 是不同动作。关闭窗口会隐藏到托盘；退出桌面端不会停止代理；Stop Proxy 必须显式确认。
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="grid grid-cols-[1fr_420px] gap-4">
              <div className="grid grid-cols-3 gap-3">
                <DangerNote title="退出应用" description="退出桌面端，代理保持运行" />
                <DangerNote title="Detach" description="隐藏窗口到托盘，不停止已有代理" />
                <DangerNote title="Stop Proxy" description="停止本地代理运行时" />
              </div>
              <div className="flex items-center justify-end gap-3">
                <Button
                  variant="outline"
                  onClick={() => runDesktopCommand(quitApp)}
                >
                  退出应用
                </Button>
                <Button
                  variant="outline"
                  onClick={() => runDesktopCommand(hideMainWindow)}
                >
                  Detach
                </Button>
                <div className="flex flex-col gap-2">
                  <Input
                    aria-label="Stop Proxy confirmation"
                    value={stopPhrase}
                    onChange={(event) => setStopPhrase(event.currentTarget.value)}
                    placeholder={runtime.state.ownerMode === "desktop-owned" ? CONTROL_CONFIRMATIONS.stopOwned : CONTROL_CONFIRMATIONS.stopAttached}
                    className="w-56 border-red-200"
                  />
                  <div className="flex gap-2">
                    <Button
                      variant="danger"
                      disabled={!runtime.state.canStopProxy || actions.stopOwned.isPending}
                      onClick={() => actions.stopOwned.mutate(stopPhrase)}
                    >
                      Stop Owned
                    </Button>
                    <Button
                      variant="warning"
                      disabled={!runtime.state.canUseLiveActions || actions.stopAttached.isPending}
                      onClick={() => actions.stopAttached.mutate(stopPhrase)}
                    >
                      Remote Stop
                    </Button>
                  </div>
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    </>
  );
}

function SettingsCard({ title, description, children }: { title: string; description: string; children: React.ReactNode }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">{children}</CardContent>
    </Card>
  );
}

function ToggleRow({ label, checked }: { label: string; checked: boolean }) {
  return (
    <FieldRow label={label}>
      <Switch checked={checked} />
    </FieldRow>
  );
}

function FieldRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex min-h-9 items-center justify-between gap-4 text-sm">
      <span className="text-slate-600">{label}</span>
      <div>{children}</div>
    </div>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <label className="space-y-1 text-xs text-slate-500">
      {label}
      <Input value={value} readOnly className="font-mono" />
    </label>
  );
}

function PathRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="grid grid-cols-[90px_1fr_auto] items-center gap-3 text-sm">
      <span className="text-slate-500">{label}</span>
      <span className="truncate font-mono text-slate-700">{value}</span>
      <Copy className="h-4 w-4 text-slate-400" />
    </div>
  );
}

function DangerNote({ title, description }: { title: string; description: string }) {
  return (
    <div className="rounded-xl bg-white/80 p-3">
      <div className="font-medium text-slate-900">{title}</div>
      <div className="mt-1 text-xs text-slate-500">{description}</div>
    </div>
  );
}
