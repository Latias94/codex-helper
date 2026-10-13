import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { UsageTable } from "@/features/usage/UsageTable";
import { mockUsageRowViews } from "@/lib/api/mock-data";
import { getRequestChain } from "@/lib/tauri/commands";

vi.mock("@/lib/tauri/commands", () => ({
  getRequestChain: vi.fn(),
}));

const row = mockUsageRowViews[0];
const mockedGetRequestChain = vi.mocked(getRequestChain);

describe("usage table", () => {
  it("renders accessor values, custom cells, and the chain action", () => {
    render(<UsageTable rows={[row]} totalRows={128} />);

    const table = screen.getByRole("table");
    const headers = within(table).getAllByRole("columnheader");
    expect(headers.map((header) => header.textContent)).toEqual([
      "API Key", "Model", "Reasoning", "Endpoint", "Type", "Billing", "Tokens",
      "预估费用", "First Token", "Duration", "Time", "Chain",
    ]);

    const cells = within(table).getAllByRole("cell");
    expect(cells).toHaveLength(headers.length);
    expect(cells[0]).toHaveTextContent(row.key);
    expect(cells[1]).toHaveTextContent(row.model);
    expect(cells[2]).toHaveTextContent(row.effort);
    expect(cells[3]).toHaveTextContent(row.endpoint);
    expect(cells[6]).toHaveTextContent(`${row.tokens.input.toLocaleString()} in ↓`);
    expect(cells[6]).toHaveTextContent(`${row.tokens.output.toLocaleString()} out ↑`);
    expect(cells[7]).toHaveTextContent(row.cost);
    expect(within(cells[11]).getByRole("button", { name: "Chain" })).toBeEnabled();
  });

  it("updates row data and renders the empty state after a refresh", () => {
    const { rerender } = render(<UsageTable rows={[row]} totalRows={1} />);

    rerender(<UsageTable rows={[{ ...row, model: "refreshed-model" }]} totalRows={1} />);
    expect(screen.getByRole("cell", { name: "refreshed-model" })).toBeInTheDocument();
    expect(screen.queryByRole("cell", { name: row.model })).not.toBeInTheDocument();

    rerender(<UsageTable rows={[]} totalRows={0} />);
    expect(screen.getByRole("cell", { name: /暂无请求历史/ })).toHaveAttribute("colspan", "12");
    expect(screen.queryByRole("button", { name: "Chain" })).not.toBeInTheDocument();
    expect(screen.getByText("最近请求 drilldown：显示 0 条；今日总请求 0 条")).toBeInTheDocument();
  });

  it("uses refreshed row data for the chain action and recovers after a failed request", async () => {
    const user = userEvent.setup();
    mockedGetRequestChain.mockRejectedValue(new Error("chain unavailable"));
    const { rerender } = render(<UsageTable rows={[row]} totalRows={1} />);

    rerender(<UsageTable rows={[{ ...row, traceId: "refreshed-trace" }]} totalRows={1} />);
    await user.click(screen.getByRole("button", { name: "Chain" }));

    expect(mockedGetRequestChain).toHaveBeenCalledWith({
      traceId: "refreshed-trace",
      requestId: undefined,
      session: undefined,
      limit: 20,
    });
    expect(await screen.findByText("chain unavailable")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Chain" })).toBeEnabled();
  });
});
