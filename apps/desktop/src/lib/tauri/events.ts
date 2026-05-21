export type DesktopLifecycleEvent =
  | { type: "runtime-started" }
  | { type: "runtime-stopped" }
  | { type: "runtime-attached" }
  | { type: "runtime-detached" };
