# Central Relay Container Runtime

Status: accepted

`codex-helper` container deployments are central relay runtimes, not Codex client-configuration owners. Container and server entrypoints never read or write Codex-owned `config.toml`, `auth.json`, model cache, or SQLite files, and they do not provide client-local transcript/session capabilities. Local `session` commands read only the Codex session files on the machine where the command runs. The separate local CLI may update only the helper provider selector/stanza in Codex `config.toml`, and only after a human explicitly runs `switch on/off`; selecting or attaching to a relay target never performs that action implicitly. This keeps LAN/Tailscale relay behavior honest for devices whose Codex files live on their own machines.

The runtime does own its helper state, including `~/.codex-helper/state/state.sqlite` (or the equivalent path under `CODEX_HELPER_HOME`). That database contains helper runtime facts; it is distinct from every Codex-owned SQLite database and must not be removed as part of client-state isolation.

A locally installed service and a Docker/host deployment are two placements of the same server product, not a local client plus a shared runtime. Each receiving server owns its own provider configuration, credential inputs, runtime store, session affinity, round-robin cursor, active-request counts, concurrency limits, health, and quota policy. Two servers may be configured for the same upstream account, but they neither coordinate nor claim a combined capacity or affinity boundary.

Credential capability follows placement. The desktop/root CLI may bind an installed same-user service to Windows Credential Manager, the macOS login Keychain, or a logged-in Linux Secret Service session. `codex-helper-server` accepts deployment-provided environment variables and absolute read-only secret-file references and rejects native references, including under Cargo feature unification. Neither placement stores upstream credential values in helper SQLite.

Remote operator clients use a `GET` / `HEAD`-only control plane and consume the typed, redacted `OperatorReadModel`. Its `ready`, `stale`, `disconnected`, and `auth_required` states are part of the runtime boundary. A client may retain the last remote model as `stale` after a refresh failure, but an offline or unauthenticated client must not synthesize runtime facts from local config, local SQLite, or an empty in-process runtime.

Fleet is an observer over those independent server snapshots. It can show each node's redacted credential readiness and freshness, but it does not route traffic, mutate credentials, transfer affinity, fail over a client, or combine cursor/concurrency/health state across nodes.
