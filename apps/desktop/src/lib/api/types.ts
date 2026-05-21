export type RuntimeMode = "running" | "attached" | "stopped" | "unavailable";

export type ProviderHealth = "healthy" | "warning" | "error" | "unknown";

export type CostEstimate = {
  amount: string;
  disclaimer: string;
};
