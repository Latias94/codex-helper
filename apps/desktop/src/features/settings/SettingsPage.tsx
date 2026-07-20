import { useState } from "react";

import { save } from "@tauri-apps/plugin-dialog";
import { Check, Copy, FolderOpen, RefreshCw } from "lucide-react";

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
import {
  CONTROL_CONFIRMATIONS,
  useDesktopControlState,
  useRuntimeActions,
} from "@/features/runtime/actions";
import { ActionStatusBanner } from "@/features/runtime/ActionStatusBanner";
import { useRuntimeSummary } from "@/features/runtime/hooks";
import { useKnownPaths, useLaunchAtLogin } from "@/features/settings/hooks";
import type {
  CodexClientPatchSnapshot,
  CodexClientPreset,
  CodexCompactionStrategy,
  CodexHostedImageGenerationMode,
  CodexSwitchPhase,
} from "@/lib/api/types";
import {
  exportConfig,
  hideMainWindow,
  openKnownPath,
  quitApp,
  type KnownPathKind,
} from "@/lib/tauri/commands";

const updatePolicy =
  "自动更新暂未启用：Tauri updater 需要签名私钥、固定公钥、HTTPS 发布端点和回滚策略；当前版本请从 GitHub Releases 手动安装。";

type CodexPresetSelection = "config" | CodexClientPreset;

type CodexPatchDraft = Omit<CodexClientPatchSnapshot, "preset"> & {
  preset: CodexPresetSelection;
};

const defaultCodexPatchDraft: CodexPatchDraft = {
  preset: "config",
  responsesWebsocket: false,
  compaction: "auto",
  translateModels: false,
  hostedImageGeneration: "auto",
};

export function SettingsPage() {
  const knownPaths = useKnownPaths();
  const launchAtLogin = useLaunchAtLogin();
  const runtime = useRuntimeSummary();
  const control = useDesktopControlState();
  const actions = useRuntimeActions();
  const [desktopStatus, setDesktopStatus] = useState<{ kind: "idle" | "success" | "error"; message: string }>({
    kind: "idle",
    message: "",
  });
  const [desktopBusy, setDesktopBusy] = useState(false);
  const [codexPatchDraft, setCodexPatchDraft] = useState<CodexPatchDraft>(defaultCodexPatchDraft);
  const paths = knownPaths.data;
  const codexSwitch = control.data?.codexSwitch;
  const codexSwitchRecovery = codexSwitch?.phase === "recovery_required";
  const codexSwitchBusy = actions.switchOn.isPending || actions.switchOff.isPending;
  const canToggleCodexSwitch = Boolean(control.data)
    && runtime.state.canUseLiveActions
    && !codexSwitchRecovery
    && !codexSwitch?.errorMessage
    && (codexSwitch?.enabled ? control.data?.canSwitchOff : control.data?.canSwitchOn);
  const canApplyCodexPreset = Boolean(control.data?.canSwitchOn)
    && runtime.state.canUseLiveActions
    && !codexSwitchRecovery
    && !codexSwitch?.errorMessage
    && !codexSwitchBusy;
  const usesConfiguredCodexPatch = codexPatchDraft.preset === "config";
  const usesOfficialCodexIdentity = codexPatchDraft.preset === "official-relay"
    || codexPatchDraft.preset === "official-imagegen";
  const canEnableResponsesWebsocket = !usesConfiguredCodexPatch
    && usesOfficialCodexIdentity
    && codexPatchDraft.compaction !== "local";
  const applyCodexPreset = () => {
    if (codexPatchDraft.preset === "config") {
      actions.switchOn.mutate({
        confirmation: CONTROL_CONFIRMATIONS.switchCodexOn,
      });
      return;
    }

    actions.switchOn.mutate({
      confirmation: CONTROL_CONFIRMATIONS.switchCodexOn,
      preset: codexPatchDraft.preset,
      responsesWebsocket: codexPatchDraft.responsesWebsocket,
      compaction: codexPatchDraft.compaction,
      translateModels: codexPatchDraft.translateModels,
      hostedImageGeneration: codexPatchDraft.hostedImageGeneration,
    });
  };
  const selectCodexPreset = (preset: CodexPresetSelection) => {
    setCodexPatchDraft((current) => {
      const official = preset === "official-relay" || preset === "official-imagegen";
      const compaction = !official && current.compaction.startsWith("remote-")
        ? "auto"
        : current.compaction;
      return {
        ...current,
        preset,
        compaction,
        responsesWebsocket: official && compaction !== "local"
          ? current.responsesWebsocket
          : false,
      };
    });
  };
  const selectCodexCompaction = (compaction: CodexCompactionStrategy) => {
    setCodexPatchDraft((current) => ({
      ...current,
      compaction,
      responsesWebsocket: compaction === "local" ? false : current.responsesWebsocket,
    }));
  };
  const runPathAction = (kind: KnownPathKind) => {
    void runDesktopAction(async () => {
      await openKnownPath({ kind });
      return `已打开 ${knownPathLabel(kind)}。`;
    });
  };
  const runExportConfig = () => {
    void runDesktopAction(async () => {
      const destination = await save({
        title: "导出 codex-helper config.toml",
        defaultPath: "codex-helper-config.toml",
        filters: [{ name: "TOML", extensions: ["toml"] }],
      });
      if (!destination) {
        return "已取消导出配置。";
      }
      return (await exportConfig({ destination })).message;
    });
  };
  const runLaunchAtLoginChange = (enabled: boolean) => {
    void runDesktopAction(async () => {
      const actual = await launchAtLogin.setEnabled(enabled);
      return actual
        ? "已启用开机启动；下次登录系统时会自动启动桌面端。"
        : "已关闭开机启动；下次登录系统时不会自动启动桌面端。";
    });
  };
  const runDesktopAction = async (action: () => Promise<string>) => {
    setDesktopBusy(true);
    setDesktopStatus({ kind: "idle", message: "" });
    try {
      const message = await action();
      setDesktopStatus({ kind: "success", message });
      await Promise.all([runtime.refetch(), knownPaths.refetch()]);
    } catch (error) {
      setDesktopStatus({ kind: "error", message: errorMessage(error) });
    } finally {
      setDesktopBusy(false);
    }
  };
  const runDesktopCommand = (command: () => Promise<unknown>) => {
    void command().catch((error) => {
      console.warn("desktop app command failed", error);
    });
  };

  return (
    <>
      <PageHeader title="设置" subtitle="配置桌面行为、本地代理和 Codex 连接" />
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
      <div className="mb-4">
        <ActionStatusBanner status={desktopStatus} busy={desktopBusy} />
      </div>

      <div className="grid grid-cols-2 gap-4">
        <SettingsCard title="桌面行为" description="控制应用启动、托盘和窗口关闭方式。">
          <ToggleRow
            label="开机启动"
            description={
              launchAtLogin.isError
                ? "当前系统暂时无法读取开机启动状态。"
                : "通过系统登录项注册桌面端，不会自动停止或重启本地代理。"
            }
            checked={launchAtLogin.data ?? false}
            disabled={launchAtLogin.isLoading || launchAtLogin.isSaving || desktopBusy}
            onCheckedChange={runLaunchAtLoginChange}
          />
          <ToggleRow label="启用托盘" description="托盘常驻已启用，关闭窗口会隐藏到托盘。" checked disabled />
          <FieldRow label="关闭窗口时">
            <Badge variant="teal">隐藏到托盘</Badge>
          </FieldRow>
          <ToggleRow
            label="启动时自动启动本地代理"
            description="保持保守默认：桌面端启动后可手动附加或启动代理，避免登录时抢占正在运行的 helper。"
            checked={false}
            disabled
          />
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
            生命周期规则：关闭窗口只隐藏到托盘，退出桌面端也不会停止代理。
          </p>
          <div className="flex gap-2 pt-2">
            <Button variant="outline"><Copy className="h-4 w-4" />复制 Endpoint</Button>
            <Button variant="outline" onClick={() => runPathAction("logs")}><FolderOpen className="h-4 w-4" />打开日志目录</Button>
          </div>
        </SettingsCard>

        <SettingsCard title="Codex 连接" description="控制 Codex 是否通过本地代理中转。">
          <ToggleRow
            label="Codex 本地中转"
            checked={codexSwitch?.enabled ?? false}
            disabled={!canToggleCodexSwitch || codexSwitchBusy}
            onCheckedChange={(enabled) => {
              if (enabled) {
                applyCodexPreset();
              } else {
                actions.switchOff.mutate(CONTROL_CONFIRMATIONS.switchCodexOff);
              }
            }}
          />
          <FieldRow label="Client preset">
            <div className="flex flex-wrap items-center justify-end gap-2">
              <SelectBox
                aria-label="Client preset"
                value={codexPatchDraft.preset}
                disabled={!canApplyCodexPreset}
                onChange={(event) => selectCodexPreset(event.currentTarget.value as CodexPresetSelection)}
                className="w-52"
              >
                <option value="config">config.toml</option>
                <option value="default">default</option>
                <option value="chatgpt-bridge">chatgpt-bridge</option>
                <option value="imagegen-bridge">imagegen-bridge</option>
                <option value="official-relay">official-relay</option>
                <option value="official-imagegen">official-imagegen</option>
              </SelectBox>
              <Button
                variant="outline"
                disabled={!canApplyCodexPreset}
                onClick={applyCodexPreset}
              >
                <Check className="h-4 w-4" />
                应用 client patch
              </Button>
            </div>
          </FieldRow>
          <FieldRow label="Compaction">
            <SelectBox
              aria-label="Compaction"
              value={codexPatchDraft.compaction}
              disabled={!canApplyCodexPreset || usesConfiguredCodexPatch}
              onChange={(event) => selectCodexCompaction(event.currentTarget.value as CodexCompactionStrategy)}
              className="w-52"
            >
              <option value="auto">auto</option>
              <option value="local">local</option>
              <option value="remote-v1" disabled={!usesOfficialCodexIdentity}>remote-v1</option>
              <option value="remote-v2" disabled={!usesOfficialCodexIdentity}>remote-v2</option>
            </SelectBox>
          </FieldRow>
          <ToggleRow
            label="Responses WebSocket"
            checked={codexPatchDraft.responsesWebsocket}
            disabled={!canApplyCodexPreset || !canEnableResponsesWebsocket}
            onCheckedChange={(responsesWebsocket) => {
              setCodexPatchDraft((current) => ({ ...current, responsesWebsocket }));
            }}
          />
          <ToggleRow
            label="Translate /models"
            checked={codexPatchDraft.translateModels}
            disabled={!canApplyCodexPreset || usesConfiguredCodexPatch}
            onCheckedChange={(translateModels) => {
              setCodexPatchDraft((current) => ({ ...current, translateModels }));
            }}
          />
          <FieldRow label="Hosted image generation">
            <SelectBox
              aria-label="Hosted image generation"
              value={codexPatchDraft.hostedImageGeneration}
              disabled={!canApplyCodexPreset || usesConfiguredCodexPatch}
              onChange={(event) => {
                const hostedImageGeneration = event.currentTarget.value as CodexHostedImageGenerationMode;
                setCodexPatchDraft((current) => ({ ...current, hostedImageGeneration }));
              }}
              className="w-52"
            >
              <option value="auto">auto</option>
              <option value="enabled">enabled</option>
              <option value="disabled">disabled</option>
            </SelectBox>
          </FieldRow>
          {codexSwitch?.clientPatch ? (
            <FieldRow label="Active patch">
              <div className="flex max-w-xl flex-wrap justify-end gap-2">
                <Badge variant="teal">{codexSwitch.clientPatch.preset}</Badge>
                <Badge variant="blue">{codexSwitch.clientPatch.compaction}</Badge>
                <Badge variant={codexSwitch.clientPatch.responsesWebsocket ? "teal" : "muted"}>
                  ws {codexSwitch.clientPatch.responsesWebsocket ? "on" : "off"}
                </Badge>
                <Badge variant={codexSwitch.clientPatch.translateModels ? "teal" : "muted"}>
                  models {codexSwitch.clientPatch.translateModels ? "translated" : "passthrough"}
                </Badge>
                <Badge variant={codexSwitch.clientPatch.hostedImageGeneration === "disabled" ? "warning" : "muted"}>
                  hosted {codexSwitch.clientPatch.hostedImageGeneration}
                </Badge>
              </div>
            </FieldRow>
          ) : null}
          <FieldRow label="Switch 阶段">
            <div className="flex items-center gap-2">
              <Badge variant={codexSwitchPhaseVariant(codexSwitch?.phase)}>
                {codexSwitch?.phase ?? "unavailable"}
              </Badge>
              <Badge variant={codexSwitch?.managed ? "teal" : "muted"}>
                {codexSwitch?.managed ? "helper 管理" : "未托管"}
              </Badge>
            </div>
          </FieldRow>
          <FieldRow label="Base URL">
            <Input
              value={codexSwitch?.baseUrl ?? "-"}
              readOnly
              className="w-72 font-mono"
            />
          </FieldRow>
          {codexSwitchRecovery ? (
            <div className="border-l-2 border-red-500 bg-red-50 px-3 py-2 text-sm text-red-800">
              <div className="font-medium">需要人工恢复</div>
              <div className="mt-1 break-words text-xs leading-5">
                {codexSwitch?.recoveryReason ?? "Codex switch 状态需要人工核对。"}
              </div>
            </div>
          ) : null}
          {codexSwitch?.errorMessage ? (
            <div className="border-l-2 border-amber-500 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-800">
              {codexSwitch.errorMessage}
            </div>
          ) : null}
        </SettingsCard>

        <SettingsCard title="关于与路径" description="版本、本机路径和更新信息。">
          <PathRow label="Version" value="v0.20.0" />
          <PathRow label="Config" value={paths?.config ?? "~/.codex-helper/config.toml"} onOpen={() => runPathAction("config")} />
          <PathRow label="Logs" value={paths?.logs ?? "~/.codex-helper/logs"} onOpen={() => runPathAction("logs")} />
          <PathRow label="Cache" value={paths?.cache ?? "~/.codex-helper/cache"} onOpen={() => runPathAction("cache")} />
          {knownPaths.isError ? (
            <div className="rounded-xl border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-700">
              暂时无法读取桌面路径，当前展示默认路径占位。
            </div>
          ) : null}
          <div className="rounded-xl border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-700">
            导出的 config.toml 可能包含 inline token；请像密钥文件一样保存。
          </div>
          <div className="rounded-xl border border-slate-200 bg-slate-50 px-3 py-2 text-xs leading-5 text-slate-600">
            {updatePolicy}
          </div>
          <div className="flex gap-2 pt-2">
            <Button variant="outline" onClick={() => runPathAction("home")}><FolderOpen className="h-4 w-4" />打开配置目录</Button>
            <Button variant="outline" onClick={runExportConfig}>导出配置</Button>
            <Button
              variant="outline"
              disabled
              title={updatePolicy}
            >
              <RefreshCw className="h-4 w-4" />
              检查更新（暂未启用）
            </Button>
          </div>
        </SettingsCard>

        <Card className="col-span-2">
          <CardHeader>
            <CardTitle>应用生命周期</CardTitle>
            <CardDescription>
              关闭窗口会隐藏到托盘；退出桌面端不会停止本地代理。
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex items-center justify-between gap-4">
              <div className="grid flex-1 grid-cols-2 gap-3">
                <LifecycleNote title="退出应用" description="退出桌面端，代理保持运行" />
                <LifecycleNote title="Detach" description="隐藏窗口到托盘，不停止已有代理" />
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

function ToggleRow({
  label,
  checked,
  description,
  disabled,
  onCheckedChange,
}: {
  label: string;
  checked: boolean;
  description?: string;
  disabled?: boolean;
  onCheckedChange?: (checked: boolean) => void;
}) {
  return (
    <FieldRow label={label}>
      <div className="flex items-center gap-3">
        {description ? (
          <span className="max-w-80 text-right text-xs leading-5 text-slate-500">{description}</span>
        ) : null}
        <Switch
          aria-label={label}
          checked={checked}
          disabled={disabled}
          onCheckedChange={onCheckedChange}
        />
      </div>
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

function PathRow({ label, value, onOpen }: { label: string; value: string; onOpen?: () => void }) {
  return (
    <div className="grid grid-cols-[90px_1fr_auto] items-center gap-3 text-sm">
      <span className="text-slate-500">{label}</span>
      <span className="truncate font-mono text-slate-700">{value}</span>
      {onOpen ? (
        <Button aria-label={`Open ${label}`} variant="ghost" className="h-8 w-8 px-0" onClick={onOpen}>
          <FolderOpen className="h-4 w-4" />
        </Button>
      ) : (
        <Copy className="h-4 w-4 text-slate-400" />
      )}
    </div>
  );
}

function LifecycleNote({ title, description }: { title: string; description: string }) {
  return (
    <div className="rounded-xl bg-white/80 p-3">
      <div className="font-medium text-slate-900">{title}</div>
      <div className="mt-1 text-xs text-slate-500">{description}</div>
    </div>
  );
}

function knownPathLabel(kind: KnownPathKind) {
  return {
    home: "配置目录",
    config: "配置文件",
    logs: "日志目录",
    cache: "缓存目录",
  }[kind];
}

function codexSwitchPhaseVariant(phase: CodexSwitchPhase | null | undefined) {
  switch (phase) {
    case "applied":
      return "success" as const;
    case "prepared":
      return "warning" as const;
    case "recovery_required":
      return "danger" as const;
    case "off":
    default:
      return "muted" as const;
  }
}

function errorMessage(error: unknown) {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "object" && error !== null) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") {
      return message;
    }
  }
  return String(error);
}
