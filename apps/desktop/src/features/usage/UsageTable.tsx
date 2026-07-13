import { createColumnHelper, flexRender, getCoreRowModel, useReactTable } from "@tanstack/react-table";
import { useState } from "react";
import { GitBranch, Info, Search } from "lucide-react";

import { Badge, Button, Card, Input, SelectBox, TooltipHint } from "@/components/ui";
import { errorToMessage } from "@/lib/api/data-state";
import type { UsageRowView } from "@/lib/api/types";
import { getRequestChain } from "@/lib/tauri/commands";
import type { ApiRequestChainExport, ApiRequestChainRequest } from "@/lib/api/admin-types";

type UsageRow = UsageRowView;

const columnHelper = createColumnHelper<UsageRow>();

const baseColumns = [
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
            <div>Input estimate: {info.row.original.costBreakdown.input}</div>
            <div>Output estimate: {info.row.original.costBreakdown.output}</div>
            <div>Cache read estimate: {info.row.original.costBreakdown.cacheRead}</div>
            <div>Cache creation estimate: {info.row.original.costBreakdown.cacheCreation}</div>
            <div>Service tier multiplier: {info.row.original.costBreakdown.serviceTierMultiplier}</div>
            <div>Provider multiplier: {info.row.original.costBreakdown.providerMultiplier}</div>
            <div>Confidence: {info.row.original.costBreakdown.confidence}</div>
            <div>Pricing source: {info.row.original.costBreakdown.source}</div>
            <div>Pricing provider: {info.row.original.costBreakdown.pricingProvider}</div>
            <div>Pricing generation: {info.row.original.costBreakdown.pricingGeneration}</div>
            <div>Pricing revision: {info.row.original.costBreakdown.effectivePricingRevision}</div>
            {info.row.original.costBreakdown.selectedTier ? (
              <div>
                Selected tier: {info.row.original.costBreakdown.selectedTier.type} · threshold{" "}
                {info.row.original.costBreakdown.selectedTier.thresholdTokens.toLocaleString()} · matched{" "}
                {info.row.original.costBreakdown.selectedTier.matchedInputTokens.toLocaleString()}
              </div>
            ) : null}
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

type ChainState =
  | { status: "idle" }
  | { status: "loading"; rowId: string }
  | { status: "success"; rowId: string; export: ApiRequestChainExport }
  | { status: "error"; rowId: string; message: string };

export function UsageTable({
  rows,
  totalRows,
  onRefresh,
}: {
  rows: UsageRow[];
  totalRows: number;
  onRefresh?: () => void;
}) {
  const [chainState, setChainState] = useState<ChainState>({ status: "idle" });
  const tableColumns = [
    ...baseColumns,
    columnHelper.display({
      id: "chain",
      header: "Chain",
      cell: (info) => {
        const row = info.row.original;
        const loading = chainState.status === "loading" && chainState.rowId === row.id;
        return (
          <Button
            className="h-8 px-2.5"
            variant="outline"
            disabled={loading}
            onClick={() => void loadRequestChain(row)}
            title="查看请求链路"
          >
            <GitBranch className="h-3.5 w-3.5" />
            {loading ? "Loading" : "Chain"}
          </Button>
        );
      },
    }),
  ];

  const table = useReactTable({
    data: rows,
    columns: tableColumns,
    getCoreRowModel: getCoreRowModel(),
  });

  async function loadRequestChain(row: UsageRow) {
    setChainState({ status: "loading", rowId: row.id });
    try {
      const requestChain = await getRequestChain({
        traceId: row.traceId,
        requestId: row.traceId ? undefined : row.requestId,
        session: row.traceId || row.requestId ? undefined : row.sessionId,
        limit: 20,
      });
      setChainState({ status: "success", rowId: row.id, export: requestChain });
    } catch (error) {
      setChainState({
        status: "error",
        rowId: row.id,
        message: errorToMessage(error) ?? "无法读取请求链路",
      });
    }
  }

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
          <Button onClick={onRefresh}>Refresh</Button>
        </div>
      </div>
      <RequestChainPanel state={chainState} />
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
            {table.getRowModel().rows.length === 0 ? (
              <tr>
                <td className="px-3 py-12 text-center text-sm text-slate-500" colSpan={tableColumns.length}>
                  暂无请求历史。Codex 请求通过本地代理后，这里会显示 request-ledger 记录。
                </td>
              </tr>
            ) : table.getRowModel().rows.map((row) => (
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
        <span>最近请求 drilldown：显示 {rows.length} 条；今日总请求 {totalRows} 条</span>
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

function RequestChainPanel({ state }: { state: ChainState }) {
  if (state.status === "idle") {
    return null;
  }

  if (state.status === "loading") {
    return (
      <div className="border-b border-slate-100 bg-slate-50 px-4 py-3 text-sm text-slate-500">
        正在读取请求链路…
      </div>
    );
  }

  if (state.status === "error") {
    return (
      <div className="border-b border-red-100 bg-red-50 px-4 py-3 text-sm text-red-700">
        {state.message}
      </div>
    );
  }

  const request = state.export.requests[0];
  if (!request) {
    return (
      <div className="border-b border-amber-100 bg-amber-50 px-4 py-3 text-sm text-amber-700">
        没有找到匹配的请求链路。
      </div>
    );
  }

  return (
    <div className="border-b border-slate-100 bg-slate-50 px-4 py-3">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="font-medium text-slate-900">Request {request.request_id}</span>
        <Badge variant={request.status_code >= 400 ? "warning" : "success"}>{request.status_code}</Badge>
        <span className="font-mono text-xs text-slate-500">{request.trace_id ?? request.session_id ?? "-"}</span>
        {state.export.truncated ? <Badge variant="warning">Truncated</Badge> : null}
      </div>
      <div className="mt-3 grid gap-3 lg:grid-cols-[1fr_1fr]">
        <ChainAttemptList request={request} />
        <ChainTimeline request={request} />
      </div>
    </div>
  );
}

function ChainAttemptList({ request }: { request: ApiRequestChainRequest }) {
  return (
    <div className="rounded-md border border-slate-200 bg-white">
      <div className="border-b border-slate-100 px-3 py-2 text-xs font-semibold uppercase text-slate-500">
        Route Attempts
      </div>
      <div className="max-h-44 overflow-auto">
        {request.route_attempts.length === 0 ? (
          <div className="px-3 py-4 text-sm text-slate-500">没有 route attempt 记录</div>
        ) : request.route_attempts.map((attempt) => (
          <div key={`${attempt.attempt_index}-${attempt.code}`} className="border-b border-slate-50 px-3 py-2 last:border-0">
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <Badge variant={attempt.status_code && attempt.status_code >= 400 ? "warning" : "muted"}>
                #{attempt.attempt_index}
              </Badge>
              <span className="font-medium text-slate-800">{attempt.code}</span>
              <span className="font-mono text-xs text-slate-500">{attempt.provider_endpoint_key ?? attempt.provider_id ?? "-"}</span>
            </div>
            <div className="mt-1 text-xs text-slate-500">
              status {attempt.status_code ?? "-"} · decision {attempt.decision} · model {attempt.model ?? request.model ?? "-"}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function ChainTimeline({ request }: { request: ApiRequestChainRequest }) {
  return (
    <div className="rounded-md border border-slate-200 bg-white">
      <div className="border-b border-slate-100 px-3 py-2 text-xs font-semibold uppercase text-slate-500">
        Timeline
      </div>
      <div className="max-h-44 overflow-auto">
        {request.timeline.map((event) => (
          <div key={`${event.order}-${event.kind}-${event.code}`} className="border-b border-slate-50 px-3 py-2 last:border-0">
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <Badge variant={event.kind === "request" ? "blue" : "muted"}>{event.kind}</Badge>
              <span className="font-medium text-slate-800">{event.code}</span>
              {event.status_code ? <span className="text-xs text-slate-500">status {event.status_code}</span> : null}
            </div>
            <div className="mt-1 font-mono text-xs text-slate-500">
              {event.provider_endpoint_key ?? event.provider_id ?? request.provider_id ?? "-"}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
