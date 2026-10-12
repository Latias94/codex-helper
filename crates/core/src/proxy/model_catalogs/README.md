# Codex model metadata

`openai-codex-0.162.1.json` contains the complete GPT-6 Astra, GPT-6 Sol,
GPT-6 Luna, and GPT-6.1 Sol entries from OpenAI Codex `rust-v0.162.1`.

Source: https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs/models-manager/models.json

The entries retain their upstream instructions, reasoning levels, context limits,
and tool and multi-agent metadata. OpenAI Codex licenses this data under Apache-2.0;
the upstream license is included in this directory.

Use these templates only when translating an OpenAI `data` list. Native Codex
`models` responses remain authoritative and pass through unchanged. Preserve the
requested slug when matching aliases; Bedrock `openai.` aliases keep the default
service tier, consistent with the existing Bedrock model compatibility policy.

To update, select the same complete entries from a pinned Codex release, update
the filename and source revision, and run the model compatibility and proxy
response regression tests. Do not replace upstream-declared capability limits
with these fallback templates.
