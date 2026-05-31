# Central Relay Container Runtime

Status: accepted

`codex-helper` container deployments are a central relay runtime, not a local client patch runtime. The container/server entrypoint must not patch `~/.codex/config.toml` or `auth.json` by default, and host-local Codex session history must be advertised only when it was explicitly mounted or enabled for that runtime. This keeps LAN/Tailscale relay behavior honest for devices whose Codex session files live on their own machines, while preserving the existing local CLI path for desktop and single-host use.
