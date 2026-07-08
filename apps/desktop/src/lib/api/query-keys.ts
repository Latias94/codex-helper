export const queryKeys = {
  appMetadata: ["desktop", "app-metadata"] as const,
  launchAtLogin: ["desktop", "launch-at-login"] as const,
  knownPaths: ["desktop", "known-paths"] as const,
  admin: {
    readModel: ["admin", "read-model"] as const,
    controlState: ["admin", "control-state"] as const,
    operatorSummary: ["admin", "operator-summary"] as const,
    runtimeStatus: ["admin", "runtime-status"] as const,
    providers: ["admin", "providers"] as const,
    requestLedgerRecent: (limit = 40) => ["admin", "request-ledger-recent", limit] as const,
    requestLedgerSummary: (by = "provider", limit = 30) =>
      ["admin", "request-ledger-summary", by, limit] as const,
    requestLedgerChain: (identity: string, limit = 20) =>
      ["admin", "request-ledger-chain", identity, limit] as const,
  },
};
