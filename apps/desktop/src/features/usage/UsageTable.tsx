import { createColumnHelper, flexRender, getCoreRowModel, useReactTable } from "@tanstack/react-table";
import { Info, Search } from "lucide-react";

import { usageRows } from "@/mocks/dashboard";
import { Badge, Button, Card, Input, SelectBox, TooltipHint } from "@/components/ui";

type UsageRow = (typeof usageRows)[number];

const columnHelper = createColumnHelper<UsageRow>();

const columns = [
  columnHelper.accessor("key", {
    header: "API Key",
    cell: (info) => <span className="font-mono text-xs">{info.getValue()}</span>,
  }),
  columnHelper.accessor("model", { header: "Model" }),
  columnHelper.accessor("effort", { header: "Reasoning" }),
  columnHelper.accessor("endpoint", {
    header: "Endpoint",
    cell: (info) => <span className="font-mono text-xs">{info.getValue()}</span>,
  }),
  columnHelper.accessor("type", { header: "Type" }),
  columnHelper.accessor("billing", { header: "Billing" }),
  columnHelper.accessor("tokens", {
    header: "Tokens",
    cell: (info) => {
      const value = info.getValue();
      return (
        <span className="text-xs">
          {value.input > 0 ? `${value.input.toLocaleString()} in ↓ · ${value.output.toLocaleString()} out ↑` : "image"}
          <span className="ml-1 text-teal-600">{value.cache} cache</span>
        </span>
      );
    },
  }),
  columnHelper.accessor("cost", {
    header: "预估费用",
    cell: (info) => (
      <TooltipHint
        content={
          <div>
            <div className="mb-1 font-semibold">预估成本明细</div>
            <div>Input estimate: $0.006</div>
            <div>Output estimate: $0.018</div>
            <div>Cache read estimate: $0.004</div>
            <div>Provider multiplier: 1.0x</div>
            <div className="mt-1 border-t border-white/20 pt-1">实际费用以供应商结算为准</div>
          </div>
        }
      >
        <span className="inline-flex items-center gap-1 font-medium text-slate-900">
          {info.getValue()}
          <Info className="h-3.5 w-3.5 text-slate-400" />
        </span>
      </TooltipHint>
    ),
  }),
  columnHelper.accessor("firstToken", { header: "First Token" }),
  columnHelper.accessor("duration", { header: "Duration" }),
  columnHelper.accessor("time", { header: "Time" }),
];

export function UsageTable() {
  const table = useReactTable({
    data: usageRows,
    columns,
    getCoreRowModel: getCoreRowModel(),
  });

  return (
    <Card className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center justify-between border-b border-slate-200 p-4">
        <div className="flex min-w-0 flex-wrap items-center gap-3">
          <SelectBox defaultValue="all">
            <option value="all">全部供应商</option>
            <option value="codex">CodeX Air</option>
          </SelectBox>
          <SelectBox defaultValue="24h">
            <option value="24h">最近 24 小时</option>
            <option value="7d">最近 7 天</option>
          </SelectBox>
          <SelectBox defaultValue="all-models">
            <option value="all-models">全部模型</option>
          </SelectBox>
          <div className="relative">
            <Search className="absolute left-3 top-2.5 h-4 w-4 text-slate-400" />
            <Input className="w-72 pl-9" placeholder="搜索 request id、模型或供应商" />
          </div>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">Reset</Button>
          <Button variant="outline">Export CSV</Button>
          <Button>Refresh</Button>
        </div>
      </div>
      <div className="app-scroll min-h-0 flex-1 overflow-auto">
        <table className="w-full min-w-[1120px] border-collapse text-left text-sm">
          <thead className="sticky top-0 z-10 bg-slate-50 text-xs uppercase tracking-wide text-slate-500 shadow-[0_1px_0_rgba(226,232,240,1)]">
            {table.getHeaderGroups().map((headerGroup) => (
              <tr key={headerGroup.id}>
                {headerGroup.headers.map((header) => (
                  <th key={header.id} className="border-b border-slate-200 px-3 py-3 font-semibold">
                    {flexRender(header.column.columnDef.header, header.getContext())}
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody>
            {table.getRowModel().rows.map((row) => (
              <tr key={row.id} className="hover:bg-mint-50/70">
                {row.getVisibleCells().map((cell) => (
                  <td key={cell.id} className="border-b border-slate-100 px-3 py-3 text-slate-700">
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="flex shrink-0 items-center justify-between border-t border-slate-100 p-4 text-sm text-slate-500">
        <span>显示 1 至 20，共 128 条</span>
        <div className="flex items-center gap-2">
          <SelectBox defaultValue="20">
            <option value="20">每页 20</option>
          </SelectBox>
          {[1, 2, 3].map((page) => (
            <Button key={page} variant={page === 1 ? "default" : "outline"} className="w-9 px-0">
              {page}
            </Button>
          ))}
          <Badge variant="muted">…</Badge>
          <Button variant="outline" className="w-9 px-0">
            7
          </Button>
        </div>
      </div>
    </Card>
  );
}
