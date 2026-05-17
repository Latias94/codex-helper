# Milestones

## M1 - Static Contract

- Config can express provider and endpoint concurrency limits.
- Route candidates carry effective limit metadata.
- Existing configs without limits preserve behavior.

## M2 - Runtime Selection

- Runtime state includes in-flight counts.
- Saturated candidates are skipped with an explicit reason.
- Saturation is independent from failure, cooldown, and balance exhaustion.

## M3 - Enforcement

- The proxy enforces limits with atomic permits.
- Streaming and non-streaming attempts release permits correctly.
- Failover can use alternate providers when the preferred candidate is full.

## M4 - Evidence

- Targeted tests cover config, selection, and execution behavior.
- Docs describe the TOML shape and local-process limitation.
