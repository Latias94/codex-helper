# Runtime Boundary Refactor - Milestones

Status: Complete
Last updated: 2026-05-31

## M1 - Docker Gate Repaired

Status: done.

Exit criteria:

- Docker image builds.
- `docker run image --help` works with the default entrypoint.
- Compose config still validates.

## M2 - Runtime Options Boundary

Status: done.

Exit criteria:

- Runtime construction uses a structured options object.
- Admin bind and advertised admin URL are represented separately.
- Existing callers preserve behavior.

## M3 - Runtime-local Capabilities

Status: done.

Exit criteria:

- Host-local policy is attached to the runtime/control-plane context.
- Server mode remains disabled by default.
- Local CLI keeps auto-detection behavior.

## M4 - Local CLI Boundary

Status: done.

Exit criteria:

- `run_server` is split into named lifecycle helpers.
- Client patching, runtime construction, TUI startup, and shutdown handling are locally understandable.
- Behavior remains covered by compile and targeted tests.

## M5 - Closeout

Status: done.

Exit criteria:

- Evidence is fresh.
- Workstream docs reflect the final state.
- Changes are committed.
