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

Physical inputs map to logical actions. Game logic never queries "is W pressed" — it queries "is move-forward active."

### Action types

| Type | Semantics | Examples |
|------|-----------|----------|
| Button | Binary on/off, with pressed/held/released states | Shoot, jump, use, reload |
| Axis | Scalar value in [-1, 1] | Move forward/back, strafe left/right, look yaw, look pitch |

A single action can have multiple physical bindings. W key and left stick Y both map to the forward/back movement axis. Bindings are data, not code.

### Axis source tagging

Axis values carry a source tag that determines how game logic integrates them:

| Source | Tag | Meaning | Integration |
|--------|-----|---------|-------------|
| Mouse delta | Displacement | Value is rotation in radians | Applied at render rate, once per frame, before the tick loop |
| Keyboard | Velocity | Value is -1, 0, or +1 | Multiply by speed and tick delta (inside the tick loop) |
| Gamepad stick (movement) | Velocity | Value is stick deflection in [-1, 1] | Multiply by speed and tick delta (inside the tick loop) |
| Gamepad stick (look) | Velocity | Value is stick deflection in [-1, 1] | Multiply by sensitivity and frame elapsed time (render rate, before tick loop) |

**Evanescent vs. persistent inputs.** Mouse displacement is evanescent — if not consumed this frame, the motion is permanently lost. It must be read at render rate, every frame, regardless of how many ticks fired. Gamepad look velocity is continuous but integrates with frame elapsed time at render rate for the same reason: tick-rate integration produces visible jitter when render rate and tick rate are not integer multiples. Held keys and movement sticks are persistent — their state is equally valid next frame — and are safely consumed inside the fixed-tick loop. This split mirrors id Tech 3's architecture: client viewangles update per rendered frame; usercmd movement integrates at tick rate.

### Binding resolution

When multiple inputs map to the same action in the same frame, the subsystem resolves them:

- **Button actions:** any bound input active means the action is active (logical OR).
- **Axis actions within the same source type:** highest-magnitude wins. Keyboard axis inputs produce -1, 0, or +1; analog stick inputs produce the stick's continuous value.
- **Axis actions across source types:** displacement and velocity are additive. When both mouse and gamepad contribute to the same look axis, both contributions are applied — they represent different physical actions (hand movement and thumb deflection) that don't conflict.

---

## 3. Frame Integration

Input runs first in the frame sequence: **Input -> Game logic -> Audio -> Render -> Present.**

Each frame, the input subsystem produces two reads:

1. Drains pending winit events (keyboard, mouse) accumulated since last frame.
2. Polls gilrs for current gamepad state.
3. **`drain_look_inputs()`** — consumes the accumulated mouse delta and current gamepad look axis values. Returns a `LookInputs` value. Clears `mouse_delta` so look input is not double-applied. Called once per frame before the tick loop.
4. **`snapshot()`** — resolves remaining bindings (movement, buttons) into an action-state snapshot. Called once per frame before the tick loop.

Look rotation is applied from `LookInputs` at render rate, once per frame, before the fixed-tick loop runs. The tick loop consumes the `snapshot` for movement and gameplay actions.

The snapshot is a read-only value. Game logic consumes it; nothing writes back to input state mid-frame.

### Mouse delta accumulation

Mouse motion events arrive between frames at OS-determined rates. The input subsystem accumulates raw deltas across all events since the last frame, then applies sensitivity and invert-Y to produce look axis values. `drain_look_inputs()` drains these values once per render frame. This guarantees no motion is lost regardless of how many fixed ticks fired in the frame — including zero.

### Render-rate look vs. tick-rate movement

View rotation (yaw, pitch) updates every render frame. Player position updates inside the fixed-tick loop at tick rate. Movement direction reads from the camera's freshest yaw — already updated before the tick loop — so movement uses the current view direction, not a lagged one.

---

## 4. Mouse Handling

| Setting | Semantics |
|---------|-----------|
| Raw motion | Look uses raw mouse deltas, not cursor position. Avoids OS acceleration curves. |
| Capture | Cursor locked and hidden during gameplay. Released for menus or when window loses focus. |
| Sensitivity | Scalar multiplier applied to raw deltas before they become look-axis values. |
| Invert Y | Negates the pitch axis. Applied after sensitivity. |

Raw mouse motion is essential for consistent aiming. OS pointer acceleration varies across platforms and user settings — raw input bypasses it.

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

## 7. Diagnostic Inputs

Diagnostics use a parallel input channel, separate from action mapping. The consumer is the engine itself — overlay toggles, per-frame trace dumps — never game logic. Gameplay actions and diagnostic chords share no namespace and never collide.

### Why a separate channel

Gameplay bindings are 1:1 inputs without modifiers. Diagnostics need modifier chords (so they can't fire by accident during play) and one-shot rising-edge semantics (one capture per press, not a state machine). Folding both into the action system would force every gameplay binding to grow modifier-aware match logic for no benefit, and dilute the gameplay action enum's role.

### Reserved namespace

`Alt+Shift+<key>` is reserved for diagnostic chords. Nothing else binds in this namespace. Two modifiers is awkward enough to prevent accidental firing during play and consistent enough to recognize at a glance.

| Key class | Use |
|---|---|
| Number row | One-shot captures (dump-this-frame). Lower digits for more frequently used captures. |
| Letters | Persistent mode toggles. |
| Symbols | Mode toggles where a symbol is a stronger mnemonic than a letter (e.g. `\|` for the wireframe overlay). |

### Chord matching

| Rule | Reason |
|---|---|
| Exact modifier match | Extra modifiers (e.g. Cmd) suppress the chord. OS shortcuts and editor binds cannot accidentally trigger diagnostics. |
| Rising edge only | Key repeats are suppressed; one press fires the action exactly once. Toggle vs. dump-on-press is the consumer's job, not the input layer's. |
| Left and right modifiers equivalent | Chords care about the modifier, not which physical key produces it. |

### Subsystem boundary

The input layer emits "user invoked this diagnostic action" and stops there. Toggle state, capture flags, and trace buffers live with the consumer (renderer, visibility stats). Diagnostic actions never map to gameplay actions and the two channels never share state.

---

## 8. Non-Goals

- Motion controls (accelerometer, gyroscope)
- Touch input
- Input recording and replay
- Networked input (prediction, rollback)
- VR/AR input (head tracking, hand controllers)
- Steam Input API integration
