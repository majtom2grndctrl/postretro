# Task 04: Gamepad Integration

> **Dependencies:** Task 02 (input core types exist)
> **Related:** `context/lib/input.md` §5 (gamepad handling contract) · `context/lib/development_guide.md` (stack: gilrs 0.11)

---

## Goal

Add gilrs-based gamepad support. Analog sticks, triggers, and buttons feed into the action snapshot from Task 02. Dead zones prevent stick drift. Gamepad and keyboard/mouse coexist -- standard binding resolution applies when both are active.

---

## Implementation Guidance

### Polling

Each frame, call `gilrs.next_event()` in a loop to drain events, then read current axis/button state from the active gamepad. gilrs must be initialized at startup alongside the winit event loop.

### Active gamepad

Track the most-recently-used gamepad. If multiple gamepads are connected, the one that last produced input is active. When no gamepad is connected, skip gamepad processing gracefully.

### Dead zones

Radial dead zone per stick. Inputs with magnitude below the threshold read as zero. Remap above-threshold range to [0, 1] so the first detectable input starts at 0, not at the dead zone edge.

**Radial dead zone formula** for a stick with axes (x, y):

1. Compute magnitude: `mag = sqrt(x*x + y*y)`.
2. If `mag < dead_zone`, output (0, 0).
3. Otherwise, normalize and remap: `output = (direction * (mag - dead_zone)) / (1.0 - dead_zone)`.
4. Clamp each axis of output to [-1, 1].

| Stick | Dead zone radius |
|-------|-----------------|
| Left stick | 0.15 |
| Right stick | 0.15 |

### Triggers

Left and right triggers are axis values in [0, 1]. Map them in two ways depending on the binding:

- As axis actions: pass the [0, 1] value directly.
- As button actions: threshold at 0.5. Above threshold = active, below = inactive.

### Binding resolution with keyboard/mouse

When both gamepad and keyboard/mouse are active in the same frame, standard resolution rules apply (Task 02): OR for buttons, highest-magnitude for axes. No special "mode switching" logic.

---

## Key Decisions

| Topic | Decision |
|-------|----------|
| Dead zone type | Radial per stick (not per-axis). Prevents diagonal dead spots. |
| Dead zone default | 0.15. Standard value across most controllers. |
| Trigger threshold | 0.5 for button interpretation. |
| Multi-gamepad | Most-recently-used wins. No split-screen or multi-player in scope. |

---

## Acceptance Criteria

1. Gamepad analog sticks produce axis values in [-1, 1] with radial dead zones applied.
2. Stick at rest (within dead zone) produces exactly zero. No drift.
3. Dead zone remapping means the first detectable input above threshold starts at 0, not at the dead zone edge.
4. Triggers produce axis values in [0, 1] and threshold correctly to button state at 0.5.
5. Gamepad buttons produce correct button action state through the binding system.
6. Simultaneous gamepad and keyboard/mouse use resolves correctly via binding resolution.
7. No gamepad connected does not crash or produce errors.
