# Task 06: Diagnostics

> **Phase:** 1 — BSP Loading and Wireframe
> **Dependencies:** task-04 (PVS culling) and task-05 (frustum culling) — this task exposes their output as observable data.
> **Produces:** debug output that makes the visibility pipeline observable and verifiable.

---

## Goal

Add diagnostic output so the visibility pipeline (PVS + frustum culling) can be verified and debugged. No text rendering — use log output and/or window title.

---

## Implementation Guidance

### Diagnostic data

Expose the following each frame:

| Metric | Source |
|--------|--------|
| Total faces in BSP | Loaded face count from task-01 |
| Faces after PVS culling | Visible face count from task-04 |
| Faces after frustum culling | Final draw count from task-05 |
| Current BSP leaf index | Point-in-leaf query from task-04 |
| Camera position | Camera state from task-03 |

### Output methods

- **Log output:** log all metrics at `debug` level per frame. Prefix with `[Diagnostics]` subsystem tag per the development guide logging rules.
- **Window title (optional):** write a summary string to the window title for quick visual feedback during development. Example: `Postretro | leaf:42 | faces: 1200/580/320 (total/pvs/frustum) | pos: (100, 50, -200)`. This avoids the need for on-screen text rendering.

### Performance consideration

Updating the window title every frame may have platform-specific overhead. If it causes issues, throttle to once per second or remove in favor of log-only output.

---

## Acceptance Criteria

1. Running with `RUST_LOG=debug cargo run -- assets/maps/test.bsp` shows per-frame diagnostic output with all five metrics.
2. Navigating between rooms shows draw counts changing in response to PVS and frustum culling.
3. Diagnostic logging does not appear at `info` level (no log spam in normal runs).
4. Window title update (if implemented) shows current stats without impacting frame rate.
