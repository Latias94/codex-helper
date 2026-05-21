import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "@/app/App";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockRejectedValue(new Error("tauri runtime unavailable in unit tests")),
}));

afterEach(() => {
  window.location.hash = "";
});

describe("desktop app routes", () => {
  it("renders the dashboard route by default", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "仪表盘" })).toBeInTheDocument();
    expect(screen.getByText("查看本地代理、Codex 连接、供应商健康和今日用量")).toBeInTheDocument();
  });

  it("renders the usage route from hash history", async () => {
    window.location.hash = "#/usage";

    render(<App />);

    expect(await screen.findByRole("heading", { name: "用量" })).toBeInTheDocument();
    expect(screen.getByText("成本展示为预估值；行内 tooltip 展示 input、output、cache read 和 multiplier 明细。")).toBeInTheDocument();
  });
});
