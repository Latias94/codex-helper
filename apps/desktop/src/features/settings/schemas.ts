import { z } from "zod";

export const localProxySettingsSchema = z.object({
  host: z.string().min(1),
  port: z.coerce.number().int().min(1).max(65_535),
});

export type LocalProxySettings = z.infer<typeof localProxySettingsSchema>;
