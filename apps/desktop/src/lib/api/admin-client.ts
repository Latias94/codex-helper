import type {
  ApiFinishedRequest,
  ApiOperatorSummary,
  ApiProviderOption,
  ApiRequestChainExport,
  ApiRequestUsageSummaryRow,
  ApiRuntimeStatus,
} from "@/lib/api/admin-types";

export const DEFAULT_PROXY_PORT = 3211;
export const ADMIN_PORT_OFFSET = 1000;
export const DEFAULT_ADMIN_BASE_URL = `http://127.0.0.1:${DEFAULT_PROXY_PORT + ADMIN_PORT_OFFSET}`;
export const DEFAULT_PROXY_BASE_URL = `http://127.0.0.1:${DEFAULT_PROXY_PORT}`;

export class AdminApiError extends Error {
  constructor(
    message: string,
    readonly status?: number,
  ) {
    super(message);
    this.name = "AdminApiError";
  }
}

export type AdminApiClientOptions = {
  baseUrl?: string;
  fetchImpl?: typeof fetch;
  timeoutMs?: number;
};

export type RequestLedgerRecentParams = {
  limit?: number;
};

export type RequestLedgerSummaryParams = {
  limit?: number;
  by?: "station" | "provider" | "model" | "session";
};

export type RequestLedgerChainParams = {
  limit?: number;
  traceId?: string;
  requestId?: number;
  session?: string;
};

export class AdminApiClient {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly timeoutMs: number;

  constructor(options: AdminApiClientOptions = {}) {
    this.baseUrl = normalizeBaseUrl(options.baseUrl ?? runtimeAdminBaseUrl());
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.timeoutMs = options.timeoutMs ?? 2_500;
  }

  getBaseUrl() {
    return this.baseUrl;
  }

  async getOperatorSummary() {
    return this.get<ApiOperatorSummary>("/__codex_helper/api/v1/operator/summary");
  }

  async getRuntimeStatus(path = "/__codex_helper/api/v1/runtime/status") {
    return this.get<ApiRuntimeStatus>(path);
  }

  async getProviders(path = "/__codex_helper/api/v1/providers") {
    return this.get<ApiProviderOption[]>(path);
  }

  async getRequestLedgerRecent(
    path = "/__codex_helper/api/v1/request-ledger/recent",
    params: RequestLedgerRecentParams = {},
  ) {
    return this.get<ApiFinishedRequest[]>(path, { limit: params.limit ?? 40 });
  }

  async getRequestLedgerSummary(
    path = "/__codex_helper/api/v1/request-ledger/summary",
    params: RequestLedgerSummaryParams = {},
  ) {
    return this.get<ApiRequestUsageSummaryRow[]>(path, {
      by: params.by ?? "provider",
      limit: params.limit ?? 30,
    });
  }

  async getRequestLedgerChain(
    path = "/__codex_helper/api/v1/request-ledger/chain",
    params: RequestLedgerChainParams = {},
  ) {
    const query: Record<string, string | number | boolean> = {
      limit: params.limit ?? 20,
    };
    if (params.traceId) {
      query.trace_id = params.traceId;
    }
    if (params.requestId !== undefined) {
      query.request_id = params.requestId;
    }
    if (params.session) {
      query.session = params.session;
    }
    return this.get<ApiRequestChainExport>(path, query);
  }

  private async get<T>(path: string, params?: Record<string, string | number | boolean>) {
    const url = buildAdminUrl(this.baseUrl, path, params);
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), this.timeoutMs);

    try {
      const response = await this.fetchImpl(url, {
        headers: { accept: "application/json" },
        signal: controller.signal,
      });

      if (!response.ok) {
        const body = await safeReadBody(response);
        throw new AdminApiError(
          body || `admin API returned HTTP ${response.status}`,
          response.status,
        );
      }

      return (await response.json()) as T;
    } catch (error) {
      if (error instanceof AdminApiError) {
        throw error;
      }
      if (error instanceof DOMException && error.name === "AbortError") {
        throw new AdminApiError(`admin API request timed out after ${this.timeoutMs}ms`);
      }
      throw new AdminApiError(error instanceof Error ? error.message : String(error));
    } finally {
      window.clearTimeout(timeout);
    }
  }
}

export function createAdminApiClient(options: AdminApiClientOptions = {}) {
  return new AdminApiClient(options);
}

export function runtimeAdminBaseUrl() {
  const fromEnv = import.meta.env.VITE_CODEX_HELPER_ADMIN_URL as string | undefined;
  return normalizeBaseUrl(fromEnv?.trim() || DEFAULT_ADMIN_BASE_URL);
}

export function adminPortForProxyPort(proxyPort: number) {
  if (proxyPort <= 65_535 - ADMIN_PORT_OFFSET) {
    return proxyPort + ADMIN_PORT_OFFSET;
  }
  if (proxyPort > ADMIN_PORT_OFFSET) {
    return proxyPort - ADMIN_PORT_OFFSET;
  }
  return 1;
}

export function proxyBaseUrlForAdminBaseUrl(adminBaseUrl: string) {
  try {
    const url = new URL(adminBaseUrl);
    const adminPort = url.port ? Number(url.port) : DEFAULT_PROXY_PORT + ADMIN_PORT_OFFSET;
    const proxyPort = adminPort > ADMIN_PORT_OFFSET ? adminPort - ADMIN_PORT_OFFSET : DEFAULT_PROXY_PORT;
    url.port = String(proxyPort);
    return normalizeBaseUrl(url.toString());
  } catch {
    return DEFAULT_PROXY_BASE_URL;
  }
}

function buildAdminUrl(
  baseUrl: string,
  path: string,
  params?: Record<string, string | number | boolean>,
) {
  const url = new URL(path, `${baseUrl}/`);
  for (const [key, value] of Object.entries(params ?? {})) {
    url.searchParams.set(key, String(value));
  }
  return url;
}

function normalizeBaseUrl(value: string) {
  return value.replace(/\/+$/, "");
}

async function safeReadBody(response: Response) {
  try {
    return await response.text();
  } catch {
    return "";
  }
}
