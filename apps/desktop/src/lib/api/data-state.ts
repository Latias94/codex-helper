import type { ApiOperatorReadModel } from "@/lib/api/admin-types";
import type { RuntimeDataState, RuntimeOwnerMode } from "@/lib/api/types";

type BuildOperatorReadModelDataStateInput = {
  model?: ApiOperatorReadModel;
  isLoading: boolean;
  isFetching: boolean;
  error?: unknown;
  ownerMode?: RuntimeOwnerMode;
};

type RuntimeIssueKind = "auth-required" | "unavailable" | "disconnected";

export function buildOperatorReadModelDataState(
  input: BuildOperatorReadModelDataStateInput,
): RuntimeDataState {
  const ownerMode = input.ownerMode ?? "unknown";
  const common = {
    ownerMode,
    canStartProxy: false,
    canAttachProxy: false,
  };

  if (input.model?.status === "ready") {
    return {
      ...common,
      status: input.isFetching ? "refreshing" : "live",
      source: "live",
      severity: input.isFetching ? "info" : "success",
      title: input.isFetching ? "正在刷新本地 admin API 数据" : "实时数据已连接",
      description: input.isFetching
        ? "当前继续显示最近一次 coherent read model。"
        : "正在读取本机 codex-helper operator read model。",
      badge: input.isFetching ? "Refreshing" : "Live",
      canUseLiveActions: true,
      isStale: false,
      lastUpdatedAt: input.model.captured_at_ms,
    };
  }

  if (input.model?.status === "stale") {
    return {
      ...common,
      status: "stale",
      source: "live",
      severity: "warning",
      title: "实时数据刷新失败，正在显示上一次成功数据",
      description: "读取操作仍可继续；provider 和配置写入已禁用，直到下一次 ready 刷新。",
      badge: "Stale data",
      canUseLiveActions: false,
      canAttachProxy: true,
      isStale: true,
      lastUpdatedAt: input.model.captured_at_ms,
      errorCode: input.model.issue,
    };
  }

  if (input.model?.status === "auth_required") {
    return unavailableState({
      ...common,
      status: "auth-required",
      severity: "danger",
      title: "需要 admin token",
      description: "本地 admin API 拒绝了当前凭证。请确认桌面进程已读取 CODEX_HELPER_ADMIN_TOKEN。",
      badge: "Admin token",
      canAttachProxy: true,
      errorCode: input.model.issue,
    });
  }

  if (input.model?.status === "disconnected") {
    return unavailableState({
      ...common,
      status: "disconnected",
      severity: "warning",
      title: "本地代理未连接",
      description: "当前没有可展示的运行时事实。请启动代理或附加已有的本地运行时。",
      badge: "Disconnected",
      canStartProxy: true,
      canAttachProxy: true,
      errorCode: input.model.issue,
    });
  }

  if (input.isLoading && !input.error) {
    return unavailableState({
      ...common,
      status: "loading",
      severity: "info",
      title: "正在连接本地 admin API",
      description: "正在读取 coherent operator read model。",
      badge: "Connecting",
    });
  }

  const errorMessage = errorToMessage(input.error);
  const errorCode = errorToCode(input.error);
  const issue = classifyRuntimeIssue(errorMessage, errorCode);
  if (issue === "auth-required") {
    return unavailableState({
      ...common,
      status: "auth-required",
      severity: "danger",
      title: "需要 admin token",
      description: "本地 admin API 拒绝了当前凭证。请确认桌面进程已读取 CODEX_HELPER_ADMIN_TOKEN。",
      badge: "Admin token",
      canAttachProxy: true,
      errorCode,
      errorMessage,
    });
  }
  if (issue === "unavailable") {
    return unavailableState({
      ...common,
      status: "unavailable",
      severity: "warning",
      title: "桌面运行时不可用",
      description: "当前环境无法调用 Tauri 命令，因此没有可展示的运行时事实。",
      badge: "Desktop unavailable",
      errorCode,
      errorMessage,
    });
  }
  return unavailableState({
    ...common,
    status: "disconnected",
    severity: "warning",
    title: "本地代理未连接",
    description: "当前没有可展示的运行时事实。请启动代理或附加已有的本地运行时。",
    badge: "Disconnected",
    canStartProxy: true,
    canAttachProxy: true,
    errorCode,
    errorMessage,
  });
}

function unavailableState(
  state: Omit<RuntimeDataState, "source" | "canUseLiveActions" | "isStale">,
): RuntimeDataState {
  return {
    ...state,
    source: "none",
    canUseLiveActions: false,
    isStale: false,
  };
}

export function errorToMessage(error: unknown): string | undefined {
  if (!error) {
    return undefined;
  }
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  if (typeof error === "object") {
    const message = Reflect.get(error, "message");
    if (typeof message === "string") {
      return message;
    }
    try {
      return JSON.stringify(error);
    } catch {
      return String(error);
    }
  }
  return String(error);
}

export function errorToCode(error: unknown): string | undefined {
  if (!error || typeof error !== "object") {
    return undefined;
  }
  const code = Reflect.get(error, "code");
  return typeof code === "string" && code.length > 0 ? code : undefined;
}

function classifyRuntimeIssue(message?: string, code?: string): RuntimeIssueKind {
  if (code === "desktop_admin_http_401" || code === "desktop_admin_http_403") {
    return "auth-required";
  }
  const text = message?.toLowerCase() ?? "";
  if (
    text.includes("admin token") ||
    text.includes("unauthorized") ||
    text.includes("forbidden") ||
    text.includes("401") ||
    text.includes("403")
  ) {
    return "auth-required";
  }
  if (
    text.includes("tauri runtime unavailable") ||
    text.includes("__tauri") ||
    text.includes("not implemented in this environment")
  ) {
    return "unavailable";
  }
  return "disconnected";
}
