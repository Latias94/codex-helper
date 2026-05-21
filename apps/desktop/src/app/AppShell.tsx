import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { emit } from "@tauri-apps/api/event";
import { useEffect } from "react";
import {
  Bell,
  ChevronDown,
  Circle,
  Minus,
  Database,
  Gauge,
  Globe2,
  Home,
  Moon,
  PanelLeftClose,
  RefreshCw,
  Settings,
  Square,
  WalletCards,
  X,
} from "lucide-react";

import { Badge, Button, Card, Separator, Switch } from "@/components/ui";
import { useRuntimeSummary } from "@/features/runtime/hooks";
import { hideMainWindow, minimizeMainWindow, toggleMainWindowMaximized } from "@/lib/tauri/commands";
import { cn } from "@/lib/utils";

const navItems = [
  { to: "/", label: "仪表盘", icon: Home },
  { to: "/providers", label: "供应商", icon: Database },
  { to: "/usage", label: "用量", icon: Gauge },
  { to: "/settings", label: "设置", icon: Settings },
] as const;

export function AppShell() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const runtime = useRuntimeSummary();
  const runtimeHealthy = runtime.source === "live" && !runtime.state.isStale;

  useEffect(() => {
    void emit("codex-helper://window-ready").catch((error) => {
      console.warn("desktop window-ready event failed", error);
    });
  }, []);

  return (
    <div className="mx-auto flex h-screen max-h-screen max-w-[1600px] overflow-hidden border-x border-slate-200/70 bg-white/35">
      <aside className="flex h-full w-64 shrink-0 flex-col border-r border-slate-200/70 bg-white/82 backdrop-blur">
        <div className="drag-region h-8 border-b border-slate-200/60 px-3 text-xs leading-8 text-slate-400">
          codex-helper
        </div>

        <div className="flex items-center gap-3 px-5 py-6">
          <div className="flex h-12 w-12 items-center justify-center rounded-2xl bg-teal-50 ring-1 ring-teal-200">
            <span className="text-xl font-black text-teal-700">C</span>
          </div>
          <div>
            <div className="font-semibold tracking-tight text-slate-950">codex-helper</div>
            <div className="text-sm text-slate-500">Local Relay Helper</div>
          </div>
        </div>

        <nav className="space-y-1 px-4">
          {navItems.map((item) => {
            const Icon = item.icon;
            const active = pathname === item.to;
            return (
              <Link
                key={item.to}
                to={item.to}
                className={cn(
                  "flex h-11 items-center gap-3 rounded-xl px-3 text-sm font-medium transition",
                  active
                    ? "bg-teal-600 text-white shadow-[0_12px_30px_rgba(15,159,143,0.24)]"
                    : "text-slate-600 hover:bg-slate-100",
                )}
              >
                <Icon className="h-5 w-5" />
                {item.label}
              </Link>
            );
          })}
        </nav>

        <div className="mt-auto space-y-4 p-4">
          <div className="flex items-center justify-between rounded-2xl px-1 py-2">
            <div className="flex items-center gap-3 text-sm font-medium text-slate-600">
              <Moon className="h-5 w-5" />
              深色模式
            </div>
            <Switch checked={false} />
          </div>

          <Card className="p-4">
            <div className="flex items-center gap-2 text-sm font-medium text-slate-700">
              <span
                className={cn(
                  "h-2.5 w-2.5 rounded-full",
                  runtimeHealthy ? "bg-emerald-500" : "bg-amber-500",
                )}
              />
              本地代理
            </div>
            <div className="mt-3 font-semibold text-teal-700">
              {runtime.data.proxy} · {runtime.data.port}
            </div>
            <div className="mt-3 flex items-center gap-2 text-sm text-slate-500">
              <Globe2 className="h-4 w-4" />
              {runtime.data.codex}
            </div>
            <Separator className="my-4" />
            <div className="flex items-center justify-between text-xs text-slate-400">
              <span>{runtime.data.version}</span>
              <span>{runtime.state.badge}</span>
            </div>
          </Card>
        </div>
      </aside>

      <main className="flex min-h-0 min-w-0 flex-1 flex-col">
        <TitleBar />
        <div className="no-drag app-scroll min-h-0 flex-1 overflow-y-auto px-7 py-6">
          <Outlet />
        </div>
      </main>
    </div>
  );
}

function TitleBar() {
  const handleWindowCommand = (command: () => Promise<unknown>) => {
    void command().catch((error) => {
      console.warn("desktop window command failed", error);
    });
  };

  return (
    <div className="drag-region flex h-8 shrink-0 items-center justify-between border-b border-slate-200/60 bg-white/55">
      <div className="ml-3 flex items-center gap-2 text-xs text-slate-400">
        <PanelLeftClose className="h-3.5 w-3.5" />
        Tauri Desktop Client
      </div>
      <div className="no-drag flex h-full items-center">
        <button
          className="flex h-full w-10 items-center justify-center text-slate-400 hover:bg-slate-100 hover:text-slate-700"
          type="button"
          aria-label="Minimize window"
          onClick={() => handleWindowCommand(minimizeMainWindow)}
        >
          <Minus className="h-3.5 w-3.5" />
        </button>
        <button
          className="flex h-full w-10 items-center justify-center text-slate-400 hover:bg-slate-100 hover:text-slate-700"
          type="button"
          aria-label="Maximize window"
          onClick={() => handleWindowCommand(toggleMainWindowMaximized)}
        >
          <Square className="h-3 w-3" />
        </button>
        <button
          className="flex h-full w-10 items-center justify-center text-slate-400 hover:bg-red-500 hover:text-white"
          type="button"
          aria-label="Close window"
          onClick={() => handleWindowCommand(hideMainWindow)}
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

export function PageHeader({
  title,
  subtitle,
  action,
}: {
  title: string;
  subtitle: string;
  action?: React.ReactNode;
}) {
  const runtime = useRuntimeSummary();
  const runtimeHealthy = runtime.source === "live" && !runtime.state.isStale;

  return (
    <div className="mb-5 flex items-start justify-between gap-4">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-slate-950">{title}</h1>
        <p className="mt-1 text-sm text-slate-500">{subtitle}</p>
      </div>
      <div className="flex items-center gap-3">
        {action ?? (
          <Button variant="outline">
            <RefreshCw className="h-4 w-4" />
            刷新
          </Button>
        )}
        <Button variant="outline" className="w-11 px-0">
          <Bell className="h-4 w-4" />
        </Button>
        <Button variant="outline">
          中文
          <ChevronDown className="h-4 w-4" />
        </Button>
        <Badge variant={runtimeHealthy ? "success" : "warning"}>
          <Circle
            className={cn(
              "h-2 w-2",
              runtimeHealthy ? "fill-emerald-500 text-emerald-500" : "fill-amber-500 text-amber-500",
            )}
          />
          {runtimeHealthy ? runtime.data.proxy : runtime.state.badge}
        </Badge>
        <Badge variant="teal">
          <WalletCards className="h-3.5 w-3.5" />
          余额 {runtime.data.balance}
        </Badge>
        <Badge variant="muted">本机</Badge>
      </div>
    </div>
  );
}
