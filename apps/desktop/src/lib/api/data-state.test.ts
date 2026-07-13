import { describe, expect, it } from "vitest";

import type { ApiOperatorReadModel } from "@/lib/api/admin-types";
import {
  buildOperatorReadModelDataState,
  errorToCode,
  errorToMessage,
} from "@/lib/api/data-state";

describe("operator read-model data state", () => {
  it.each([
    ["ready", "live", true, false],
    ["stale", "stale", false, true],
    ["disconnected", "disconnected", false, false],
    ["auth_required", "auth-required", false, false],
  ] as const)(
    "derives %s from the server status",
    (operatorStatus, expectedStatus, canUseLiveActions, isStale) => {
      const state = buildOperatorReadModelDataState({
        model: operatorModel(operatorStatus),
        isFetching: false,
        isLoading: false,
      });

      expect(state.status).toBe(expectedStatus);
      expect(state.canUseLiveActions).toBe(canUseLiveActions);
      expect(state.isStale).toBe(isStale);
      expect(state.lastUpdatedAt).toBe(
        operatorStatus === "disconnected" || operatorStatus === "auth_required" ? undefined : 1234,
      );
    },
  );

  it("does not promote disconnected data to ready because the query succeeded", () => {
    const state = buildOperatorReadModelDataState({
      model: operatorModel("disconnected"),
      isFetching: false,
      isLoading: false,
    });

    expect(state.status).toBe("disconnected");
    expect(state.source).toBe("none");
    expect(state.canUseLiveActions).toBe(false);
  });

  it("keeps server-retained stale data visible while disabling writes", () => {
    const model = operatorModel("stale");
    const state = buildOperatorReadModelDataState({
      model,
      isFetching: false,
      isLoading: false,
    });

    expect(model.data).toBeDefined();
    expect(model.revisions).toBeDefined();
    expect(state.status).toBe("stale");
    expect(state.canUseLiveActions).toBe(false);
  });

  it("keeps a Tauri runtime failure fact-free", () => {
    const state = buildOperatorReadModelDataState({
      error: new Error("tauri runtime unavailable in unit tests"),
      isFetching: false,
      isLoading: false,
    });

    expect(state.status).toBe("unavailable");
    expect(state.source).toBe("none");
    expect(state.title).not.toContain("示例数据");
  });

  it("normalizes structured command failures", () => {
    const error = { code: "desktop_admin_http_403", message: "forbidden" };
    expect(errorToCode(error)).toBe("desktop_admin_http_403");
    expect(errorToMessage(error)).toBe("forbidden");
  });
});

function operatorModel(status: ApiOperatorReadModel["status"]): ApiOperatorReadModel {
  const base = {
    api_version: 1 as const,
    service_name: "codex",
    captured_at_ms: status === "disconnected" || status === "auth_required" ? 0 : 1234,
  };
  if (status === "disconnected") {
    return { ...base, status, issue: "disconnected" };
  }
  if (status === "auth_required") {
    return { ...base, status, issue: "auth_required" };
  }
  const facts = {
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
    data: {} as never,
  };
  return status === "ready"
    ? { ...base, ...facts, status }
    : { ...base, ...facts, status, issue: "refresh_failed" };
}
