# Codex model metadata

`openai-codex-0.162.1.json` contains all 11 complete model entries from OpenAI
Codex `rust-v0.162.1`, including the GPT-6 and GPT-5.6 families, GPT-5.5,
Daybreak aliases, and Codex Auto Review.

Source: https://github.com/openai/codex/blob/092d3acd6bec3e3a14bdc7e7a2810ab628ab759d/codex-rs/models-manager/models.json

The entries retain their upstream instructions, reasoning levels, context limits,
and tool and multi-agent metadata. OpenAI Codex licenses this data under Apache-2.0;
the upstream license is included in this directory.

Merge native Codex `models` catalogs and OpenAI `data` lists by their input
members. A known official slug uses its complete bundled record as the baseline;
an unknown model overlays its first native record onto a complete ID-based
fallback. Missing native reasoning levels do not inherit inferred efforts.
Only supported reasoning levels are unioned with upstream declarations, in the
client's effort order. Other capabilities and context limits stay with the
selected baseline. Explicit API exclusions and hidden entries filter only their
own source. ID-only entries retain the fallback's existing visibility policy,
including hidden image models. Unknown models with an invalid default reasoning
effort select the first merged effort, or no default when the effort list is empty.

Preserve the requested slug and its casing when matching exact official names
and aliases. Bedrock `openai.` aliases keep the default service tier, consistent
with the existing Bedrock model compatibility policy. The bundled catalog does
not introduce models that were absent from the input catalogs.

To update, copy the complete catalog from a pinned Codex release, update
the filename and source revision, and run the model compatibility and proxy
response regression tests. Retain the upstream source revision and license.
