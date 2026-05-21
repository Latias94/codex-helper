import { z } from "zod";

export const providerCredentialSchema = z.object({
  name: z.string().min(1),
  host: z.string().min(1),
  authSource: z.string().min(1),
});

export type ProviderCredentialForm = z.infer<typeof providerCredentialSchema>;
