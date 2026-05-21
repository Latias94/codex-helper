import { describe, expect, it } from "vitest";

import { buildRuntimeDataState, errorToMessage } from "@/lib/api/data-state";

describe("runtime data state", () => {
  it("shows a loading connection state before the first read model resolves", () => {
    const state = buildRuntimeDataState({
      hasLiveData: false,
      isLoading: true,
      isFetching: true,
    });

    expect(state.status).toBe("loading");
    expect(state.title).toContain("正在连接本地 admin API");
    expect(state.isFallback).toBe(true);
  });

  it("classifies browser or Vitest previews as desktop-runtime unavailable mock fallback", () => {
    const state = buildRuntimeDataState({
      hasLiveData: false,
      isLoading: false,
      isFetching: false,
      error: new Error("tauri runtime unavailable in unit tests"),
    });

    expect(state.status).toBe("unavailable");
    expect(state.title).toContain("当前展示离线示例数据");
    expect(state.badge).toBe("Desktop unavailable");
  });

  it("classifies missing admin token separately from network disconnects", () => {
    const state = buildRuntimeDataState({
      hasLiveData: false,
      isLoading: false,
      isFetching: false,
      error: "HTTP 403 forbidden: missing x-codex-helper-admin-token",
    });

    expect(state.status).toBe("auth-required");
    expect(state.title).toBe("需要 admin token");
    expect(state.canAttachProxy).toBe(true);
    expect(state.canStartProxy).toBe(false);
  });

  it("teaches the user what to do when the local proxy is disconnected", () => {
    const state = buildRuntimeDataState({
      hasLiveData: false,
      isLoading: false,
      isFetching: false,
      error: new Error("admin API http://127.0.0.1:4211 is not reachable: connection refused"),
    });

    expect(state.status).toBe("disconnected");
    expect(state.description).toContain("启动代理");
    expect(state.canStartProxy).toBe(true);
    expect(state.canAttachProxy).toBe(true);
  });

  it("keeps previous live data visible but disables live actions after a failed refresh", () => {
    const state = buildRuntimeDataState({
      hasLiveData: true,
      isLoading: false,
      isFetching: false,
      error: new Error("refresh timed out"),
    });

    expect(state.status).toBe("stale");
    expect(state.source).toBe("live");
    expect(state.isFallback).toBe(false);
    expect(state.isStale).toBe(true);
    expect(state.canUseLiveActions).toBe(false);
  });

  it("normalizes non-Error command failures into readable messages", () => {
    expect(errorToMessage({ message: "command failed" })).toBe("command failed");
    expect(errorToMessage("plain failure")).toBe("plain failure");
  });
});
