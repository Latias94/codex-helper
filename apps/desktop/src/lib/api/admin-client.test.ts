import { describe, expect, it, vi } from "vitest";

import {
  AdminApiClient,
  adminPortForProxyPort,
  proxyBaseUrlForAdminBaseUrl,
} from "@/lib/api/admin-client";

describe("AdminApiClient", () => {
  it("builds admin API urls with query params", async () => {
    const fetchImpl = vi.fn().mockResolvedValue(
      new Response(JSON.stringify([]), {
        headers: { "content-type": "application/json" },
        status: 200,
      }),
    );
    const client = new AdminApiClient({
      baseUrl: "http://127.0.0.1:4211/",
      fetchImpl,
    });

    await client.getRequestLedgerRecent(undefined, { limit: 25 });

    expect(fetchImpl).toHaveBeenCalledWith(
      new URL("http://127.0.0.1:4211/__codex_helper/api/v1/request-ledger/recent?limit=25"),
      expect.objectContaining({ headers: { accept: "application/json" } }),
    );
  });

  it("maps proxy ports to admin ports with the core offset rule", () => {
    expect(adminPortForProxyPort(3211)).toBe(4211);
    expect(adminPortForProxyPort(65_000)).toBe(64_000);
  });

  it("derives the proxy base URL from an admin base URL", () => {
    expect(proxyBaseUrlForAdminBaseUrl("http://127.0.0.1:4211")).toBe("http://127.0.0.1:3211");
  });
});
