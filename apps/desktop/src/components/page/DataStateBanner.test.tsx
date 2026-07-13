import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { DataStateBanner } from "@/components/page/DataStateBanner";
import type { RuntimeDataState } from "@/lib/api/types";

describe("DataStateBanner", () => {
  it("does not render for healthy live data", () => {
    const { container } = render(<DataStateBanner state={state({ status: "live", source: "live" })} />);

    expect(container).toBeEmptyDOMElement();
  });

  it("renders auth-token-required guidance with retry action", () => {
    const onRefresh = vi.fn();

    render(
      <DataStateBanner
        state={state({
          status: "auth-required",
          severity: "danger",
          title: "需要 admin token",
          description: "请确认 CODEX_HELPER_ADMIN_TOKEN 已配置。",
          badge: "Admin token",
        })}
        onRefresh={onRefresh}
      />,
    );

    expect(screen.getByText("需要 admin token")).toBeInTheDocument();
    expect(screen.getByText(/CODEX_HELPER_ADMIN_TOKEN/)).toBeInTheDocument();
    expect(screen.getByText("Admin token")).toBeInTheDocument();
  });

  it("renders stale-runtime copy without falling back to mock data", () => {
    render(
      <DataStateBanner
        state={state({
          status: "stale",
          source: "live",
          severity: "warning",
          title: "实时数据刷新失败，正在显示上一次成功数据",
          description: "重试刷新或检查本地代理运行时。",
          badge: "Stale data",
          isStale: true,
        })}
      />,
    );

    expect(screen.getByText("实时数据刷新失败，正在显示上一次成功数据")).toBeInTheDocument();
    expect(screen.getByText("Stale data")).toBeInTheDocument();
    expect(screen.queryByText("Mock fallback")).not.toBeInTheDocument();
  });
});

function state(overrides: Partial<RuntimeDataState>): RuntimeDataState {
  return {
    status: "disconnected",
    source: "none",
    severity: "warning",
    title: "本地代理未连接",
    description: "当前没有可展示的运行时事实。",
    badge: "Disconnected",
    canUseLiveActions: false,
    canStartProxy: true,
    canAttachProxy: true,
    isStale: false,
    ownerMode: "unknown",
    ...overrides,
  };
}
