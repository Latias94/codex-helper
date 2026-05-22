import { z } from "zod";

export const providerCredentialSchema = z.object({
  name: z.string().min(1),
  host: z.string().min(1),
  authSource: z.string().min(1),
});

export const providerCommonEditSchema = z.object({
  service: z.enum(["codex", "claude"]),
  providerName: z.string().trim().min(1, "providerName 不能为空"),
  alias: z.string().trim(),
  baseUrl: z.string().trim().refine(isHttpUrl, "Base URL 必须是 http(s) 绝对地址"),
  enabled: z.boolean(),
  authTokenEnv: z.string().trim().optional(),
  apiKeyEnv: z.string().trim().optional(),
});

function isHttpUrl(value: string) {
  try {
    const parsed = new URL(value);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

export type ProviderCredentialForm = z.infer<typeof providerCredentialSchema>;
