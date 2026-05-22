import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "@/app/App";
import { queryClient } from "@/app/query-client";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockRejectedValue(new Error("tauri runtime unavailable in unit tests")),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
  save: vi.fn().mockResolvedValue(null),
}));

const mockedInvoke = vi.mocked(invoke);

beforeEach(() => {
  queryClient.clear();
  mockedInvoke.mockRejectedValue(new Error("tauri runtime unavailable in unit tests"));
});

afterEach(() => {
  queryClient.clear();
  window.location.hash = "";
});

describe("desktop app routes", () => {
  it("renders the dashboard route by default", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "仪表盘" })).toBeInTheDocument();
    expect(screen.getByText("查看本地代理、Codex 连接、供应商健康和今日用量")).toBeInTheDocument();
    expect(await screen.findByText(/当前展示离线示例数据/)).toBeInTheDocument();
  });

  it("renders the usage route from hash history", async () => {
    window.location.hash = "#/usage";

    render(<App />);

    expect(await screen.findByRole("heading", { name: "用量" })).toBeInTheDocument();
    expect(await screen.findByText(/当前展示离线示例数据/)).toBeInTheDocument();
    expect(screen.getByText("成本展示为预估值；行内 tooltip 展示 input、output、cache read 和 multiplier 明细。")).toBeInTheDocument();
  });

  it("renders an admin-token-required state when the local admin API rejects credentials", async () => {
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.16.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        throw new Error("HTTP 403 forbidden: missing x-codex-helper-admin-token");
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("需要 admin token")).toBeInTheDocument();
    expect(screen.getByText(/CODEX_HELPER_ADMIN_TOKEN/)).toBeInTheDocument();
  });

  it("renders an empty usage state when live admin data has no request records", async () => {
    window.location.hash = "#/usage";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.16.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel({
          providers: [],
          recentRequests: [],
          usageSummary: [],
        });
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("实时数据已连接，但当前没有请求历史")).toBeInTheDocument();
    expect(screen.getByText(/先让 Codex 通过本地代理发起一次请求/)).toBeInTheDocument();
  });

  it("renders live admin data when the Tauri read model command succeeds", async () => {
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.16.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel();
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("gpt-live")).toBeInTheDocument();
    expect(screen.getAllByText("live-provider").length).toBeGreaterThan(0);
    expect(screen.queryByText("当前展示离线示例数据")).not.toBeInTheDocument();
  });

  it("routes the custom close button to hide-to-tray instead of quitting the proxy", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "仪表盘" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Close window" }));

    expect(mockedInvoke).toHaveBeenCalledWith("hide_main_window");
    expect(mockedInvoke).not.toHaveBeenCalledWith("stop_proxy", expect.anything());
  });

  it("keeps Settings Quit App and Detach separate from Stop Proxy", async () => {
    window.location.hash = "#/settings";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.16.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel();
      }
      if (command === "get_desktop_control_state") {
        return liveControlState();
      }
      if (command === "get_known_paths") {
        return {
          home: "C:/Users/dev",
          config: "C:/Users/dev/.codex-helper/config.toml",
          logs: "C:/Users/dev/.codex-helper/logs",
          cache: "C:/Users/dev/.codex-helper/cache",
        };
      }
      if (command === "hide_main_window" || command === "quit_app") {
        return undefined;
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByRole("heading", { name: "设置" })).toBeInTheDocument();
    expect(screen.getAllByText(/退出桌面端不会停止代理/).length).toBeGreaterThanOrEqual(1);

    await userEvent.click(screen.getByRole("button", { name: "Detach" }));
    await userEvent.click(screen.getByRole("button", { name: "退出应用" }));

    expect(mockedInvoke).toHaveBeenCalledWith("hide_main_window");
    expect(mockedInvoke).toHaveBeenCalledWith("quit_app");
    expect(mockedInvoke).not.toHaveBeenCalledWith("stop_proxy", expect.anything());
  });

  it("wires Settings path and config backup actions to desktop commands", async () => {
    const dialog = await import("@tauri-apps/plugin-dialog");
    vi.mocked(dialog.save).mockResolvedValue("C:/Users/dev/Desktop/config-export.toml");
    vi.mocked(dialog.open).mockResolvedValue("C:/Users/dev/Desktop/import.toml");

    window.location.hash = "#/settings";
    mockedInvoke.mockImplementation(async (command, args) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.16.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel();
      }
      if (command === "get_desktop_control_state") {
        return liveControlState();
      }
      if (command === "get_known_paths") {
        return {
          home: "C:/Users/dev/.codex-helper",
          config: "C:/Users/dev/.codex-helper/config.toml",
          logs: "C:/Users/dev/.codex-helper/logs",
          cache: "C:/Users/dev/.codex-helper/cache",
        };
      }
      if (command === "open_known_path") {
        return undefined;
      }
      if (command === "export_config") {
        expect(args).toEqual({ payload: { destination: "C:/Users/dev/Desktop/config-export.toml" } });
        return {
          ok: true,
          action: "export-config",
          message: "已导出当前 codex-helper config.toml；如果文件中包含 inline token，请按密钥文件保管。",
          source: "C:/Users/dev/.codex-helper/config.toml",
          destination: "C:/Users/dev/Desktop/config-export.toml",
          secretWarning: true,
        };
      }
      if (command === "import_config") {
        expect(args).toEqual({ payload: { source: "C:/Users/dev/Desktop/import.toml" } });
        return {
          ok: true,
          action: "import-config",
          message: "已导入 config.toml；如本地代理正在运行，请重新加载运行时配置。",
          source: "C:/Users/dev/Desktop/import.toml",
          destination: "C:/Users/dev/.codex-helper/config.toml",
          backup: "C:/Users/dev/.codex-helper/config.toml.1779410000.bak",
          secretWarning: true,
        };
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByRole("heading", { name: "设置" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "打开配置目录" }));
    await userEvent.click(screen.getByRole("button", { name: "导出配置" }));
    await userEvent.click(screen.getByRole("button", { name: "导入配置" }));

    expect(mockedInvoke).toHaveBeenCalledWith("open_known_path", { payload: { kind: "home" } });
    expect(mockedInvoke).toHaveBeenCalledWith("export_config", {
      payload: { destination: "C:/Users/dev/Desktop/config-export.toml" },
    });
    expect(mockedInvoke).toHaveBeenCalledWith("import_config", {
      payload: { source: "C:/Users/dev/Desktop/import.toml" },
    });
    expect(await screen.findByText(/已备份当前配置到/)).toBeInTheDocument();
  });
});

function liveReadModel(overrides?: {
  providers?: Array<unknown>;
  recentRequests?: Array<unknown>;
  usageSummary?: Array<unknown>;
}) {
  const providers = overrides?.providers ?? [
    {
      name: "live-provider",
      configured_enabled: true,
      effective_enabled: true,
      routable_endpoints: 1,
      endpoints: [
        {
          provider_name: "live-provider",
          name: "default",
          base_url: "https://live.example/v1",
          configured_enabled: true,
          effective_enabled: true,
          routable: true,
          runtime_state: "normal",
        },
      ],
    },
  ];

  return {
    endpoint: {
      proxyPort: 3211,
      adminPort: 4211,
      proxyBaseUrl: "http://127.0.0.1:3211",
      adminBaseUrl: "http://127.0.0.1:4211",
    },
    operatorSummary: {
      api_version: 1,
      service_name: "codex",
      runtime: {
        runtime_loaded_at_ms: Date.now(),
        effective_active_station: "live-provider",
        default_profile: "chatgpt-bridge",
        default_profile_summary: { name: "chatgpt-bridge", station: "live-provider" },
      },
      counts: { providers: providers.length, recent_requests: overrides?.recentRequests?.length ?? 1 },
      retry: { upstream_max_attempts: 1, provider_max_attempts: 1 },
      health: {},
      providers,
    },
    runtimeStatus: {
      runtime_source_path: "config.toml",
      config_path: "config.toml",
      loaded_at_ms: Date.now(),
      shutdown_available: true,
    },
    providers: overrides?.providers ?? [],
    recentRequests: overrides?.recentRequests ?? [
      {
        id: 99,
        trace_id: "codex-live-99",
        model: "gpt-live",
        provider_id: "live-provider",
        usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 },
        cost: { total_cost_usd: "0.0005", confidence: "estimated" },
        service: "codex",
        method: "POST",
        path: "/v1/responses",
        status_code: 200,
        duration_ms: 800,
        ttfb_ms: 200,
        streaming: true,
        ended_at_ms: Date.now(),
      },
    ],
    usageSummary: overrides?.usageSummary ?? [],
  };
}

function liveControlState() {
  return {
    connectionMode: "attached",
    proxyPort: 3211,
    adminPort: 4211,
    proxyBaseUrl: "http://127.0.0.1:3211",
    adminBaseUrl: "http://127.0.0.1:4211",
    reachable: true,
    shutdownAvailable: true,
    owner: null,
    codexSwitch: {
      enabled: true,
      modelProvider: "codex-helper",
      providerName: "live-provider",
      baseUrl: "http://127.0.0.1:3211/v1",
      preset: "chatgpt-bridge",
      requiresOpenaiAuth: false,
      supportsWebsockets: true,
      remoteCompactionV2Enabled: true,
      hasSwitchState: true,
      errorMessage: null,
    },
    canStart: false,
    canAttach: true,
    canStopOwned: false,
    canRemoteStop: true,
    canSwitchOn: true,
    canSwitchOff: true,
  };
}
