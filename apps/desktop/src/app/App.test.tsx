import { render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "@/app/App";
import { queryClient } from "@/app/query-client";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockRejectedValue(new Error("tauri runtime unavailable in unit tests")),
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
    expect(await screen.findByText("当前展示离线示例数据")).toBeInTheDocument();
  });

  it("renders the usage route from hash history", async () => {
    window.location.hash = "#/usage";

    render(<App />);

    expect(await screen.findByRole("heading", { name: "用量" })).toBeInTheDocument();
    expect(await screen.findByText("当前展示离线示例数据")).toBeInTheDocument();
    expect(screen.getByText("成本展示为预估值；行内 tooltip 展示 input、output、cache read 和 multiplier 明细。")).toBeInTheDocument();
  });

  it("renders live admin data when the Tauri read model command succeeds", async () => {
    mockedInvoke.mockImplementation(async (command) => {
      if (command === "get_app_metadata") {
        return { name: "codex-helper", version: "0.16.0", tauri: "2" };
      }
      if (command === "get_admin_read_model") {
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
            counts: { providers: 1, recent_requests: 1 },
            retry: { upstream_max_attempts: 1, provider_max_attempts: 1 },
            health: {},
            providers: [
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
            ],
          },
          runtimeStatus: {
            runtime_source_path: "config.toml",
            config_path: "config.toml",
            loaded_at_ms: Date.now(),
            shutdown_available: true,
          },
          providers: [],
          recentRequests: [
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
          usageSummary: [],
        };
      }
      throw new Error(`unexpected command ${command}`);
    });

    render(<App />);

    expect(await screen.findByText("gpt-live")).toBeInTheDocument();
    expect(screen.getAllByText("live-provider").length).toBeGreaterThan(0);
    expect(screen.queryByText("当前展示离线示例数据")).not.toBeInTheDocument();
  });
});
