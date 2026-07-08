import type { DataSource, RuntimeDataState, RuntimeOwnerMode } from "@/lib/api/types";

type BuildRuntimeDataStateInput = {
  hasLiveData: boolean;
  isLoading: boolean;
  isFetching: boolean;
  error?: unknown;
  isEmpty?: boolean;
  ownerMode?: RuntimeOwnerMode;
  lastUpdatedAt?: number;
};

type RuntimeIssueKind = "auth-required" | "unavailable" | "disconnected" | "unknown";

const LIVE_STATE: RuntimeDataState = {
  status: "live",
  source: "live",
  severity: "success",
  title: "实时数据已连接",
  description: "正在读取本机 codex-helper admin API。",
  badge: "Live",
  canUseLiveActions: true,
  canStartProxy: false,
  canAttachProxy: false,
  canStopProxy: false,
  isFallback: false,
  isStale: false,
  ownerMode: "unknown",
};

export function buildRuntimeDataState(input: BuildRuntimeDataStateInput): RuntimeDataState {
  const errorMessage = errorToMessage(input.error);
  const errorCode = errorToCode(input.error);
  const ownerMode = input.ownerMode ?? "unknown";
  const base = {
    ownerMode,
    lastUpdatedAt: input.lastUpdatedAt,
    errorCode,
    errorMessage,
  };

  if (input.hasLiveData && errorMessage) {
    return {
      ...base,
      status: "stale",
      source: "live",
      severity: "warning",
      title: "实时数据刷新失败，正在显示上一次成功数据",
      description: "可以先继续查看当前数据；如果需要执行控制动作，请重试刷新或检查本地代理运行时。",
      badge: "Stale data",
      canUseLiveActions: false,
      canStartProxy: false,
      canAttachProxy: true,
      canStopProxy: false,
      isFallback: false,
      isStale: true,
    };
  }

  if (input.hasLiveData && input.isFetching) {
    return {
      ...LIVE_STATE,
      ...base,
      status: "refreshing",
      severity: "info",
      title: "正在刷新本地 admin API 数据",
      description: "当前仍显示实时数据；刷新失败时会保留上一份可用数据并标记为 stale。",
      badge: "Refreshing",
    };
  }

  if (input.hasLiveData && input.isEmpty) {
    return {
      ...LIVE_STATE,
      ...base,
      status: "empty",
      severity: "neutral",
      title: "实时数据已连接，但当前没有业务记录",
      description: "先让 Codex 通过本地代理发起一次请求，或在 Providers 中配置可路由供应商。",
      badge: "Empty",
    };
  }

  if (input.hasLiveData) {
    return {
      ...LIVE_STATE,
      ...base,
    };
  }

  if (input.isLoading && !errorMessage) {
    return {
      ...base,
      status: "loading",
      source: "mock",
      severity: "info",
      title: "正在连接本地 admin API",
      description: "正在尝试读取 127.0.0.1 的 codex-helper 运行时；连接完成后会自动切换为实时数据。",
      badge: "Connecting",
      canUseLiveActions: false,
      canStartProxy: false,
      canAttachProxy: false,
      canStopProxy: false,
      isFallback: true,
      isStale: false,
    };
  }

  const issue = classifyRuntimeIssue(errorMessage, errorCode);

  if (issue === "auth-required") {
    return {
      ...base,
      status: "auth-required",
      source: "mock",
      severity: "danger",
      title: "需要 admin token",
      description:
        "本地 admin API 要求携带 token。请确认桌面端已读取 CODEX_HELPER_ADMIN_TOKEN，并在 Tauri 命令中注入 x-codex-helper-admin-token。",
      badge: "Admin token",
      canUseLiveActions: false,
      canStartProxy: false,
      canAttachProxy: true,
      canStopProxy: false,
      isFallback: true,
      isStale: false,
    };
  }

  if (issue === "unavailable") {
    return {
      ...base,
      status: "unavailable",
      source: "mock",
      severity: "warning",
      title: "桌面运行时不可用，当前展示离线示例数据",
      description: "当前可能在浏览器或 Vitest 中预览，无法调用 Tauri 命令；请在 Tauri 窗口中启动或附加本地代理。",
      badge: "Desktop unavailable",
      canUseLiveActions: false,
      canStartProxy: false,
      canAttachProxy: false,
      canStopProxy: false,
      isFallback: true,
      isStale: false,
    };
  }

  if (issue === "disconnected") {
    return {
      ...base,
      status: "disconnected",
      source: "mock",
      severity: "warning",
      title: "本地代理未连接，当前展示离线示例数据",
      description: "没有连到 127.0.0.1:4211。可以先启动代理，或在下一阶段使用 Attach Existing 附加已有运行时。",
      badge: "Disconnected",
      canUseLiveActions: false,
      canStartProxy: true,
      canAttachProxy: true,
      canStopProxy: false,
      isFallback: true,
      isStale: false,
    };
  }

  return {
    ...base,
    status: "mock",
    source: "mock",
    severity: "neutral",
    title: "当前展示离线示例数据",
    description: "启动或附加 codex-helper 本地代理后会自动切换为实时数据。",
    badge: "Mock fallback",
    canUseLiveActions: false,
    canStartProxy: true,
    canAttachProxy: true,
    canStopProxy: false,
    isFallback: true,
    isStale: false,
  };
}

export function deriveDataSource(state: RuntimeDataState): DataSource {
  return state.source;
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
  if (typeof error === "object" && error !== null) {
    const maybeMessage = (error as { message?: unknown }).message;
    if (typeof maybeMessage === "string") {
      return maybeMessage;
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
  const maybeCode = (error as { code?: unknown }).code;
  return typeof maybeCode === "string" && maybeCode.length > 0 ? maybeCode : undefined;
}

function classifyRuntimeIssue(message: string | undefined, code?: string): RuntimeIssueKind {
  if (code === "desktop_admin_http_401" || code === "desktop_admin_http_403") {
    return "auth-required";
  }
  if (
    code === "desktop_admin_connection_failed" ||
    code === "desktop_admin_timeout" ||
    code === "desktop_admin_request_failed"
  ) {
    return "disconnected";
  }
  if (code === "desktop_admin_decode_error" || code === "desktop_admin_http_status") {
    return "disconnected";
  }

  if (!message) {
    return "unknown";
  }

  const text = message.toLowerCase();

  if (
    text.includes("x-codex-helper-admin-token") ||
    text.includes("codex_helper_admin_token") ||
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

  if (
    text.includes("connection refused") ||
    text.includes("econnrefused") ||
    text.includes("not reachable") ||
    text.includes("failed to fetch") ||
    text.includes("networkerror") ||
    text.includes("timed out") ||
    text.includes("timeout") ||
    text.includes("127.0.0.1") ||
    text.includes("4211")
  ) {
    return "disconnected";
  }

  return "disconnected";
}
