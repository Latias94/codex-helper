import { AlertTriangle, KeyRound, Loader2, PlugZap, RotateCw, Wifi, WifiOff } from "lucide-react";

import { Badge, Button } from "@/components/ui";
import type { DataSource, RuntimeDataState } from "@/lib/api/types";
import { cn } from "@/lib/utils";

const bannerTone = {
  neutral: "border-slate-200 bg-white/80 text-slate-700",
  info: "border-sky-200 bg-sky-50/80 text-sky-800",
  success: "border-emerald-200 bg-emerald-50/80 text-emerald-800",
  warning: "border-amber-200 bg-amber-50/80 text-amber-800",
  danger: "border-red-200 bg-red-50/80 text-red-800",
} as const;

const descriptionTone = {
  neutral: "text-slate-500",
  info: "text-sky-700",
  success: "text-emerald-700",
  warning: "text-amber-700",
  danger: "text-red-700",
} as const;

export function DataStateBanner({
  state,
  source,
  isLoading,
  isRefreshing,
  errorMessage,
  onRefresh,
}: {
  state?: RuntimeDataState;
  source?: DataSource;
  isLoading?: boolean;
  isRefreshing?: boolean;
  errorMessage?: string;
  onRefresh?: () => void;
}) {
  const bannerState =
    state ??
    legacyState({
      source: source ?? "mock",
      isLoading: Boolean(isLoading),
      isRefreshing: Boolean(isRefreshing),
      errorMessage,
    });

  if (bannerState.status === "live") {
    return null;
  }

  const Icon = iconForStatus(bannerState.status);
  const loading = bannerState.status === "loading" || bannerState.status === "refreshing";

  return (
    <div
      className={cn(
        "mb-4 flex items-center justify-between gap-4 rounded-2xl border px-4 py-3 text-sm",
        bannerTone[bannerState.severity],
      )}
    >
      <div className="flex min-w-0 items-center gap-3">
        {loading ? (
          <Loader2 className="h-4 w-4 animate-spin" />
        ) : (
          <Icon className="h-4 w-4" />
        )}
        <div className="min-w-0">
          <div className="font-medium">{bannerState.title}</div>
          <div className={cn("truncate text-xs", descriptionTone[bannerState.severity])}>
            {bannerState.description}
            {bannerState.errorMessage ? `（${bannerState.errorMessage}）` : ""}
          </div>
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <Badge variant={badgeVariantForState(bannerState)}>
          <Wifi className="h-3.5 w-3.5" />
          {bannerState.badge}
        </Badge>
        {onRefresh ? (
          <Button variant="outline" onClick={onRefresh}>
            重试刷新
          </Button>
        ) : null}
      </div>
    </div>
  );
}

function iconForStatus(status: RuntimeDataState["status"]) {
  if (status === "auth-required") {
    return KeyRound;
  }
  if (status === "disconnected" || status === "unavailable") {
    return WifiOff;
  }
  if (status === "stale") {
    return RotateCw;
  }
  if (status === "empty") {
    return PlugZap;
  }
  return AlertTriangle;
}

function badgeVariantForState(state: RuntimeDataState) {
  if (state.status === "auth-required") {
    return "danger" as const;
  }
  if (state.status === "refreshing" || state.status === "stale") {
    return "blue" as const;
  }
  if (state.source === "live") {
    return "success" as const;
  }
  if (state.status === "mock" || state.status === "empty") {
    return "muted" as const;
  }
  return "warning" as const;
}

function legacyState({
  source,
  isLoading,
  isRefreshing,
  errorMessage,
}: {
  source: DataSource;
  isLoading: boolean;
  isRefreshing: boolean;
  errorMessage?: string;
}): RuntimeDataState {
  return {
    status: source === "live" ? (isRefreshing ? "refreshing" : "live") : isLoading ? "loading" : "mock",
    source,
    severity: source === "live" ? "info" : "warning",
    title: source === "live" ? "正在刷新本地 admin API 数据" : "当前展示离线示例数据",
    description: errorMessage
      ? `无法连接本地 admin API：${errorMessage}`
      : "启动或附加 codex-helper 本地代理后会自动切换为实时数据。",
    badge: source === "live" ? "Live" : "Mock fallback",
    canUseLiveActions: source === "live",
    canStartProxy: source !== "live",
    canAttachProxy: source !== "live",
    canStopProxy: false,
    isFallback: source !== "live",
    isStale: false,
    ownerMode: "unknown",
    errorMessage,
  };
}
