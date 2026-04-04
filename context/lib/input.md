# Input

> **Read this when:** building or modifying the input subsystem, adding new player actions, or changing how game logic consumes input.
> **Key invariant:** game logic reads an action-state snapshot each frame, never raw input events. Keyboard, mouse, and gamepad are interchangeable at the action layer.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md)

---

## 1. Input Sources

| Source | Crate | Role |
|--------|-------|------|
| Keyboard / mouse | winit 0.30 | winit owns the event loop; input subsystem processes its events |
| Gamepad | gilrs 0.11 | Cross-platform gamepad polling, analog sticks, triggers |

winit delivers keyboard and mouse events through the event loop it already owns. gilrs polls gamepad state independently. Both feed into the same action-mapping layer.

---

## 2. Action Mapping

Physical inputs map to logical actions. Game logic never queries "is W pressed" -- it queries "is move-forward active."

### Action types

| Type | Semantics | Examples |
|------|-----------|----------|
| Button | Binary on/off, with pressed/held/released states | Shoot, jump, use, reload |
| Axis | Scalar value in [-1, 1] | Move forward/back, strafe left/right, look yaw, look pitch |

A single action can have multiple physical bindings. W key and left stick Y both map to the forward/back movement axis. Bindings are data, not code -- stored in configuration, rebindable at runtime.

### Binding resolution

When multiple inputs map to the same action in the same frame, the subsystem resolves them:

- **Button actions:** any bound input active means the action is active (logical OR).
- **Axis actions:** highest-magnitude input wins. Keyboard axis inputs produce -1, 0, or +1; analog stick inputs produce the stick's continuous value.

---

## 3. Frame Integration

Input runs first in the frame sequence: **Input -> Game logic -> Audio -> Render -> Present.**

Each frame, the input subsystem:

1. Drains pending winit events (keyboard, mouse) accumulated since last frame.
2. Polls gilrs for current gamepad state.
3. Resolves all bindings into an action-state snapshot.
4. Hands the snapshot to game logic.

The snapshot is a read-only value. Game logic consumes it; nothing writes back to input state mid-frame.

### Mouse delta accumulation

Mouse motion events arrive between frames at OS-determined rates. The input subsystem accumulates raw deltas across all events since the last frame, then applies sensitivity and invert-Y to produce the final look axis values. This prevents lost motion at low framerates and jitter at high framerates.

---

## 4. Mouse Handling

| Setting | Semantics |
|---------|-----------|
| Raw motion | Look uses raw mouse deltas, not cursor position. Avoids OS acceleration curves. |
| Capture | Cursor locked and hidden during gameplay. Released for menus or when window loses focus. |
| Sensitivity | Scalar multiplier applied to raw deltas before they become look-axis values. |
| Invert Y | Negates the pitch axis. Applied after sensitivity. |

Raw mouse motion is essential for consistent aiming. OS pointer acceleration varies across platforms and user settings -- raw input bypasses it.

---

## 5. Gamepad Handling

gilrs provides a unified gamepad API across platforms.

| Concern | Approach |
|---------|----------|
| Dead zones | Per-stick radial dead zone. Inputs below the threshold read as zero. Configurable per player preference. |
| Triggers | Analog axis in [0, 1]. Map to axis actions (e.g., analog acceleration) or threshold to button actions. |
| Action parity | Gamepad bindings map to the same actions as keyboard/mouse. Switching input device mid-play requires no mode change. |

Gamepad and keyboard/mouse bindings coexist. If both are active in the same frame, binding resolution (section 2) applies.

---

## 6. Subsystem Boundary

The input subsystem produces one thing: an action-state snapshot per frame. Game logic is its only consumer.

| Boundary rule | Rationale |
|---------------|-----------|
| Snapshot is the only output | Game logic depends on action semantics, not input hardware |
| No wgpu dependency | Input has no rendering concern. Keeps the module testable without a GPU context. |
| No reverse dependency | Game logic never pushes state back into input mid-frame. Information flows one direction. |
| Configurable bindings are input's concern | Game logic does not know which key maps to which action |

---

## 7. Non-Goals

- Motion controls (accelerometer, gyroscope)
- Touch input
- Input recording and replay
- Networked input (prediction, rollback)
- VR/AR input (head tracking, hand controllers)
- Steam Input API integration
