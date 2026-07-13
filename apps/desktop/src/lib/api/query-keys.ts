export const queryKeys = {
  appMetadata: ["desktop", "app-metadata"] as const,
  launchAtLogin: ["desktop", "launch-at-login"] as const,
  knownPaths: ["desktop", "known-paths"] as const,
  admin: {
    readModel: ["admin", "read-model"] as const,
    controlState: ["admin", "control-state"] as const,
    requestLedgerChain: (identity: string, limit = 20) =>
      ["admin", "request-ledger-chain", identity, limit] as const,
  },
};
