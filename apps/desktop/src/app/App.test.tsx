import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "@/app/App";
import { queryClient } from "@/app/query-client";
import type { ApiUsageDayView } from "@/lib/api/admin-types";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockRejectedValue(new Error("tauri runtime unavailable in unit tests")),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn().mockResolvedValue(null),
}));

vi.mock("@tauri-apps/plugin-autostart", () => ({
  disable: vi.fn().mockResolvedValue(undefined),
  enable: vi.fn().mockResolvedValue(undefined),
  isEnabled: vi.fn().mockResolvedValue(false),
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
    expect(screen.getByText("查看本地代理、Codex 连接、供应商路由状态和今日用量")).toBeInTheDocument();
    expect(await screen.findByText("桌面运行时不可用")).toBeInTheDocument();
    expect(screen.queryByText("Running")).not.toBeInTheDocument();
    expect(screen.queryByText("CodeX Air")).not.toBeInTheDocument();
  });

  it("renders the usage route from hash history", async () => {
    window.location.hash = "#/usage";

    render(<App />);

    expect(await screen.findByRole("heading", { name: "用量" })).toBeInTheDocument();
    expect(await screen.findByText("桌面运行时不可用")).toBeInTheDocument();
    expect(screen.getByText("成本展示为预估值；行内 tooltip 展示 input、output、cache read 和 multiplier 明细。")).toBeInTheDocument();
  });

  it("renders an admin-token-required state when the local admin API rejects credentials", async () => {
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return unavailableReadModel("auth_required");
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("需要 admin token")).toBeInTheDocument();
    expect(screen.getByText(/CODEX_HELPER_ADMIN_TOKEN/)).toBeInTheDocument();
    expect(screen.queryByText("Running")).not.toBeInTheDocument();
    expect(screen.queryByText("live-provider")).not.toBeInTheDocument();
  });

  it("keeps stale provider facts visible without exposing writes", async () => {
    window.location.hash = "#/providers";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return staleReadModel();
      }
      if (command === "get_desktop_control_state") {
        return liveControlState();
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("实时数据刷新失败，正在显示上一次成功数据")).toBeInTheDocument();
    expect(screen.getAllByText("live-provider").length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /编辑 live-provider/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "添加供应商" })).not.toBeInTheDocument();
  });

  it("renders an empty usage state when live admin data has no daily usage", async () => {
    window.location.hash = "#/usage";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel({
          providers: [],
          recentRequests: [],
        });
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("实时数据已连接，但今天还没有用量")).toBeInTheDocument();
    expect(screen.getByText(/usage_day 写入后统计和 drilldown 会自动更新/)).toBeInTheDocument();
  });

  it("renders live admin data when the Tauri read model command succeeds", async () => {
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
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

  it("renders single-endpoint provider facts without a config editor", async () => {
    window.location.hash = "#/providers";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel({
          providers: [
            {
              name: "single-provider",
              alias: "Single Provider",
              configured_enabled: true,
              effective_enabled: true,
              routable_endpoints: 1,
              endpoints: [
                {
                  provider_name: "single-provider",
                  name: "default",
                  provider_endpoint_key: "endpoint:sha256:single",
                  origin: "https://old.example",
                  priority: 0,
                  configured_enabled: true,
                  effective_enabled: true,
                  routable: true,
                  runtime_state: "normal",
                },
              ],
            },
          ],
        });
      }
      if (command === "get_desktop_control_state") {
        return liveControlState();
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByRole("heading", { name: "供应商" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "刷新余额" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Probe" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Set Active" })).not.toBeInTheDocument();
    expect(screen.getByText("https://old.example")).toBeInTheDocument();
    expect(screen.getByText("1/1")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /编辑 Single Provider/ })).not.toBeInTheDocument();
    expect(mockedInvoke).not.toHaveBeenCalledWith("save_common_provider", expect.anything());
  });

  it("renders multi-endpoint providers as read-only facts", async () => {
    window.location.hash = "#/providers";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel({
          providers: [
            {
              name: "multi-provider",
              configured_enabled: true,
              effective_enabled: true,
              routable_endpoints: 2,
              endpoints: [
                {
                  provider_name: "multi-provider",
                  name: "primary",
                  provider_endpoint_key: "endpoint:sha256:primary",
                  origin: "https://primary.example",
                  priority: 0,
                  configured_enabled: true,
                  effective_enabled: true,
                  routable: true,
                  runtime_state: "normal",
                },
                {
                  provider_name: "multi-provider",
                  name: "backup",
                  provider_endpoint_key: "endpoint:sha256:backup",
                  origin: "https://backup.example",
                  priority: 1,
                  configured_enabled: true,
                  effective_enabled: true,
                  routable: true,
                  runtime_state: "normal",
                },
              ],
            },
          ],
        });
      }
      if (command === "get_desktop_control_state") {
        return liveControlState();
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByRole("heading", { name: "供应商" })).toBeInTheDocument();
    expect(await screen.findByText("2/2")).toBeInTheDocument();
    expect(screen.getByText("2 endpoints")).toBeInTheDocument();
    expect(screen.getByText("https://primary.example")).toBeInTheDocument();
    expect(screen.getByText("https://backup.example")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /编辑 multi-provider/ })).not.toBeInTheDocument();
  });

  it("routes the custom close button to hide-to-tray instead of quitting the proxy", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "仪表盘" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Close window" }));

    expect(mockedInvoke).toHaveBeenCalledWith("hide_main_window");
    expect(mockedInvoke).not.toHaveBeenCalledWith("stop_proxy", expect.anything());
  });

  it("keeps local Settings lifecycle actions after remote controls are removed", async () => {
    window.location.hash = "#/settings";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
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
    expect(screen.getAllByText(/退出桌面端.*不会停止.*代理/).length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByRole("button", { name: "重新加载运行时" })).not.toBeInTheDocument();
    expect(screen.queryByText("全局路由覆盖")).not.toBeInTheDocument();
    expect(screen.queryByText("会话路由覆盖")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop Owned" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remote Stop" })).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Detach" }));
    await userEvent.click(screen.getByRole("button", { name: "退出应用" }));

    expect(mockedInvoke).toHaveBeenCalledWith("hide_main_window");
    expect(mockedInvoke).toHaveBeenCalledWith("quit_app");
    expect(mockedInvoke).not.toHaveBeenCalledWith("stop_proxy", expect.anything());
  });

  it("shows Codex switch recovery state without offering an unsafe overwrite", async () => {
    window.location.hash = "#/settings";
    const recoveryState = {
      ...liveControlState(),
      codexSwitch: {
        phase: "recovery_required",
        enabled: false,
        managed: true,
        baseUrl: "http://127.0.0.1:3211/v1",
        recoveryReason: "Codex config changed after switch on",
        errorMessage: null,
      },
    };
    mockedInvoke.mockImplementation(async (command, args) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel();
      }
      if (command === "get_desktop_control_state") {
        return recoveryState;
      }
      if (command === "get_known_paths") {
        return {
          home: "C:/Users/dev",
          config: "C:/Users/dev/.codex-helper/config.toml",
          logs: "C:/Users/dev/.codex-helper/logs",
          cache: "C:/Users/dev/.codex-helper/cache",
        };
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("需要人工恢复")).toBeInTheDocument();
    expect(screen.getByText("Codex config changed after switch on")).toBeInTheDocument();
    expect(screen.queryByText("当前预设")).not.toBeInTheDocument();
    expect(screen.queryByText("chatgpt-bridge")).not.toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Codex 本地中转" })).toBeDisabled();
  });

  it("sends only the canonical local Codex switch payload", async () => {
    window.location.hash = "#/settings";
    const offState = {
      ...liveControlState(),
      codexSwitch: {
        phase: "off",
        enabled: false,
        managed: false,
        baseUrl: null,
        recoveryReason: null,
        errorMessage: null,
      },
    };
    mockedInvoke.mockImplementation(async (command, args) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
        return liveReadModel();
      }
      if (command === "get_desktop_control_state") {
        return offState;
      }
      if (command === "get_known_paths") {
        return {
          home: "C:/Users/dev",
          config: "C:/Users/dev/.codex-helper/config.toml",
          logs: "C:/Users/dev/.codex-helper/logs",
          cache: "C:/Users/dev/.codex-helper/cache",
        };
      }
      if (command === "switch_codex") {
        expect(args).toEqual({
          payload: {
            enabled: true,
            confirmation: "SWITCH CODEX",
          },
        });
        return {
          ok: true,
          action: "switch-codex-on",
          message: "Codex local switch applied (phase: applied).",
          state: {
            ...offState,
            codexSwitch: {
              ...offState.codexSwitch,
              phase: "applied",
              enabled: true,
              managed: true,
              baseUrl: "http://127.0.0.1:3211/v1",
            },
          },
        };
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    await userEvent.click(screen.getByRole("switch", { name: "Codex 本地中转" }));

    expect(mockedInvoke).toHaveBeenCalledWith("switch_codex", {
      payload: {
        enabled: true,
        confirmation: "SWITCH CODEX",
      },
    });
  });

  it("keeps Settings path and config export actions without config import", async () => {
    const dialog = await import("@tauri-apps/plugin-dialog");
    vi.mocked(dialog.save).mockResolvedValue("C:/Users/dev/Desktop/config-export.toml");

    window.location.hash = "#/settings";
    mockedInvoke.mockImplementation(async (command, args) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
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
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByRole("heading", { name: "设置" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "打开配置目录" }));
    await userEvent.click(screen.getByRole("button", { name: "导出配置" }));

    expect(mockedInvoke).toHaveBeenCalledWith("open_known_path", { payload: { kind: "home" } });
    expect(mockedInvoke).toHaveBeenCalledWith("export_config", {
      payload: { destination: "C:/Users/dev/Desktop/config-export.toml" },
    });
    expect(screen.queryByRole("button", { name: "导入配置" })).not.toBeInTheDocument();
    expect(mockedInvoke).not.toHaveBeenCalledWith("import_config", expect.anything());
    expect(await screen.findByText(/已导出当前 codex-helper config.toml/)).toBeInTheDocument();
  });

  it("wires Settings launch-at-login toggle to the autostart plugin", async () => {
    const autostart = await import("@tauri-apps/plugin-autostart");
    vi.mocked(autostart.isEnabled)
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce(true)
      .mockResolvedValue(true);

    window.location.hash = "#/settings";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
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
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    const launchAtLoginSwitch = await screen.findByRole("switch", { name: "开机启动" });
    expect(launchAtLoginSwitch).toHaveAttribute("aria-checked", "false");

    await userEvent.click(launchAtLoginSwitch);

    expect(autostart.enable).toHaveBeenCalledOnce();
    expect(autostart.disable).not.toHaveBeenCalled();
    expect(await screen.findByText(/已启用开机启动/)).toBeInTheDocument();
  });

  it("shows honest disabled update posture until signing and release hosting are ready", async () => {
    window.location.hash = "#/settings";
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.20.0", tauri: "2" };
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
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    const updateButton = await screen.findByRole("button", { name: "检查更新（暂未启用）" });
    expect(updateButton).toBeDisabled();
    expect(screen.getByText(/自动更新暂未启用/)).toBeInTheDocument();
    expect(screen.getByText(/签名私钥、固定公钥、HTTPS 发布端点和回滚策略/)).toBeInTheDocument();
  });
});

function liveReadModel(overrides?: {
  providers?: Array<unknown>;
  recentRequests?: Array<unknown>;
  usageDay?: ApiUsageDayView;
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
          provider_endpoint_key: "endpoint:sha256:live",
          origin: "https://live.example",
          priority: 0,
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
    operatorReadModel: {
      api_version: 1,
      service_name: "codex",
      status: "ready",
      captured_at_ms: Date.now(),
      revisions: {
        runtime_revision: 1,
        runtime_digest: "runtime",
        route_digest: "route",
        catalog_revision: "catalog",
        pricing_revision: "pricing",
        operator_pricing_revision: "operator-pricing",
        policy_revision: 2,
        ledger_revision: "operator-ledger-v1:test",
      },
      data: {
        summary: {
          api_version: 1,
          service_name: "codex",
          runtime: {
            runtime_loaded_at_ms: Date.now(),
            runtime_source_mtime_ms: null,
            configured_default_profile: "chatgpt-bridge",
            default_profile: "chatgpt-bridge",
            default_profile_summary: {
              name: "chatgpt-bridge",
              model: null,
              reasoning_effort: null,
              service_tier: null,
              fast_mode: false,
            },
          },
          counts: {
            active_requests: 0,
            providers: providers.length,
            recent_requests: overrides?.recentRequests?.length ?? 1,
            sessions: 0,
            profiles: 0,
          },
          retry: {
            configured_profile: "balanced",
            upstream_max_attempts: 1,
            provider_max_attempts: 1,
            recent_retried_requests: 0,
            recent_cross_provider_failovers: 0,
            recent_same_provider_retries: 0,
            recent_fast_mode_requests: 0,
          },
          sessions: [],
          profiles: [],
          providers,
        },
        active_requests: [],
        recent_requests: overrides?.recentRequests ?? [
          {
            id: 99,
            model: "gpt-live",
            provider_id: "live-provider",
            usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 },
            cost: { total_cost_usd: "0.0005", confidence: "estimated" },
            observability: {
              attempt_count: 1,
              route_attempt_count: 1,
              retried: false,
              cross_provider_failover: false,
              same_provider_retry: false,
              fast_mode: false,
              streaming: true,
            },
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
        usage_summaries: [
          {
            group: "provider_endpoint",
            coverage: {
              source: "runtime_store",
              first_terminal_at_ms: null,
              last_terminal_at_ms: null,
              requests: 0,
              all_history: true,
            },
            rows: [],
          },
        ],
        usage_day:
          overrides?.usageDay ?? usageDayFixture(overrides?.recentRequests?.length ?? 1),
        usage_rollup: {},
        stats_5m: {},
        stats_1h: {},
        pricing_catalog: { source: "bundled", model_count: 0, models: [] },
        provider_balances: [],
      },
    },
  };
}

function usageDayFixture(requests: number): ApiUsageDayView {
  const inputTokens = requests * 100;
  const outputTokens = requests * 20;
  return {
    day: 0,
    label: "today",
    start_ms: 0,
    end_ms: 86_400_000,
    generated_at_ms: Date.now(),
    summary: {
      requests_total: requests,
      requests_error: 0,
      duration_ms_total: requests * 800,
      requests_with_usage: requests,
      duration_ms_with_usage_total: requests * 800,
      generation_ms_total: requests * 600,
      ttfb_ms_total: requests * 200,
      ttfb_samples: requests,
      usage: {
        input_tokens: inputTokens,
        output_tokens: outputTokens,
        reasoning_tokens: 0,
        total_tokens: inputTokens + outputTokens,
      },
      cost: {
        confidence: "unknown",
        priced_requests: 0,
        unpriced_requests: requests,
      },
    },
    hourly: [],
    provider_rows: [],
    provider_endpoint_rows: [],
    model_rows: [],
    session_rows: [],
    project_rows: [],
    retry_gate: {
      active: 0,
      active_cooldowns: 0,
      max_remaining_secs: null,
      reasons: [],
    },
    coverage: {
      source: "runtime_store",
      loaded_first_ms: null,
      loaded_last_ms: null,
      loaded_requests: requests,
      day_may_be_partial: false,
    },
  };
}

function staleReadModel() {
  const model = liveReadModel();
  return {
    ...model,
    operatorReadModel: {
      ...model.operatorReadModel,
      status: "stale",
      issue: "refresh_failed",
    },
  };
}

function unavailableReadModel(status: "disconnected" | "auth_required") {
  return {
    endpoint: {
      proxyPort: 3211,
      adminPort: 4211,
      proxyBaseUrl: "http://127.0.0.1:3211",
      adminBaseUrl: "http://127.0.0.1:4211",
    },
    operatorReadModel: {
      api_version: 1,
      service_name: "codex",
      status,
      captured_at_ms: 0,
      issue: status,
    },
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
    owner: null,
    codexSwitch: {
      phase: "applied",
      enabled: true,
      managed: true,
      baseUrl: "http://127.0.0.1:3211/v1",
      recoveryReason: null,
      errorMessage: null,
    },
    canStart: false,
    canAttach: true,
    canSwitchOn: true,
    canSwitchOff: true,
  };
}
