# Task 02: Input Core Types and Action Snapshot

> **Dependencies:** none
> **Related:** `context/lib/input.md` (action mapping contract) · `context/lib/development_guide.md` §4.3 (frame ordering)

---

## Goal

Build the action-mapping layer: action types, binding resolution, and the per-frame action snapshot. This is the foundation all input sources (keyboard, mouse, gamepad) feed into. Game logic reads the snapshot, never raw input events.

---

## Implementation Guidance

### Core types

| Type | Role |
|------|------|
| Action ID | Enum of all logical actions: MoveForward, MoveRight, MoveUp, LookYaw, LookPitch, Sprint, Jump, Use, Shoot, AltFire, Reload. Define all known actions now even if unused -- avoids breaking changes when later phases add consumers. |
| Button state | Pressed (just activated), Held (still active), Released (just deactivated), Inactive. |
| Axis value | f32 in [-1, 1]. |
| Binding | Maps a physical input (key, mouse axis, gamepad button/axis) to an action ID. |
| Action snapshot | Complete read-only state for all actions in one frame. |

### Button state transitions

| Previous | Current input | New state |
|----------|--------------|-----------|
| Inactive | Active | Pressed |
| Pressed | Active | Held |
| Held | Active | Held |
| Held | Inactive | Released |
| Released | Inactive | Inactive |
| Pressed | Inactive | Released |

### Binding resolution

When multiple inputs map to the same action in a single frame:

- **Button actions:** any bound input active = action active (logical OR).
- **Axis actions:** highest-magnitude input wins. Keyboard axis inputs produce -1, 0, or +1; analog inputs produce continuous values.

### Axis source tagging

Axis values carry a source tag: displacement (mouse delta) or velocity (gamepad stick). Mouse look is displacement-based (axis value = rotation in radians, apply directly). Gamepad look is velocity-based (axis value = rotation speed, multiply by tick delta). Tag each axis value with its source type when writing it into the snapshot. Task-06 (action camera) consumes these tags to apply the correct look model.

**Concurrent input resolution:** When both mouse and gamepad contribute to the same look axis in the same frame, they are additive rather than competing — mouse displacement is applied first, then gamepad velocity contribution is added. These represent different physical actions (a hand movement and a thumb deflection) and don't conflict. The "highest-magnitude wins" rule applies only within the same source type (e.g., two keyboard bindings on the same axis).

### Per-frame flow

1. Drain winit events accumulated since last frame. Update per-key pressed/released tracking.
2. Accumulate mouse deltas (detailed in Task 03).
3. Poll gilrs for gamepad state (detailed in Task 04).
4. Resolve all bindings into the action snapshot.
5. Hand snapshot to game logic. Snapshot is immutable for the rest of the frame.

For this task, implement the winit keyboard event processing (step 1) and the binding resolution + snapshot production (step 4). Mouse and gamepad feed in from Tasks 03 and 04. Design the input subsystem entry point so those tasks can plug in without restructuring.

### Physical input representation

The binding type needs to represent:

- Keyboard keys (winit `KeyCode`)
- Mouse buttons (winit `MouseButton`)
- Mouse axes (X delta, Y delta)
- Gamepad buttons (gilrs `Button`)
- Gamepad axes (gilrs `Axis`)

Use an enum with variants for each source type.

### Module structure

Input subsystem lives in `src/input/`. Entry point is `mod.rs` or a barrel file. Internal types are `pub(crate)` or private. The action snapshot type is public -- game logic depends on it.

---

## Key Decisions

| Topic | Decision |
|-------|----------|
| Action enum completeness | Define all known actions now, even if unused. |
| Binding storage | Hardcoded constants for Phase 2. Config file and rebinding UI are out of scope. |
| Snapshot ownership | Snapshot is produced by input, moved to game logic. No shared references. |

---

## Acceptance Criteria

1. Action ID enum covers all actions listed above.
2. Button state machine correctly transitions through Pressed -> Held -> Released -> Inactive.
3. Binding resolution produces correct results: OR for buttons, highest-magnitude for axes.
4. Keyboard input (winit events) produces correct action state for bound keys.
5. Action snapshot is immutable once produced -- game logic cannot write back to it.
6. Input subsystem is structured so mouse (Task 03) and gamepad (Task 04) plug in without restructuring the core.
7. Unit tests cover all six button state transitions. Unit tests verify binding resolution: OR across multiple button bindings, highest-magnitude across multiple axis bindings, and additive cross-source resolution for look axes.
