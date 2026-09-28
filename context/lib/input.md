# Input

> **Read this when:** building or modifying the input subsystem, adding new player actions, changing how game logic consumes input, or routing UI navigation, the accessibility panel's global input, or rebinding.
> **Key invariant:** game logic reads an action-state snapshot each frame, never raw input events. Keyboard, mouse, and gamepad are interchangeable at the action layer.
> **Related:** [Architecture Index](./index.md) · [Development Guide](./development_guide.md) · [Player Options](./player_options.md) §6 (bindings) · [UI](./ui.md) §4

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

A single action can have multiple physical bindings. W key and left stick Y both map to the forward/back movement axis. Bindings are data, not code. Decided, not yet built: they are player data — remappable and persisted per player, UI navigation included (`player_options.md` §6).

**Button signal width.** Each consumer chooses its own signal width when reading a button action from the snapshot. `is_active()` (Pressed|Held) is a level signal — it fires on every qualifying tick while the button is held. `ButtonState::Pressed` alone is a rising edge — it fires only on the first tick. Use a rising edge when a held input would wrongly re-trigger each qualifying tick: dash uses `ButtonState::Pressed` because a held dash would re-fire every cooldown-ready tick. Jump uses the level signal (`is_active()`) because the movement system self-gates it via a ceiling rule.

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
3. **`drain_look_inputs()`** — consumes accumulated look contributions from both input source classes and clears them so they are not double-applied. Called once per frame before the tick loop.
4. **`snapshot()`** — resolves remaining bindings (movement, buttons) into an action-state snapshot. Called once per frame before the tick loop.

Look rotation is applied from `drain_look_inputs()` at render rate, once per frame, before the fixed-tick loop runs. The result sums two additive source classes: mouse motion contributes a per-frame displacement (already a delta — apply as-is), and gamepad stick contributes a continuous velocity that is integrated over frame elapsed time. Both classes drive look simultaneously; neither zeroes the other. The tick loop consumes the `snapshot` for movement and gameplay actions.

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

## 5. Input Focus

`InputFocus` is the single source of truth for pointer-lock state and event gating. Defined in `input/focus.rs`.

| Variant | Cursor | Owner |
|---------|--------|-------|
| `Gameplay` | Locked and hidden | Player input / action system |
| `DevTools` | Released | Debug overlay (egui) |
| `Menu` | Released | Modal UI stack (consumer lands with E13 input breadth — a capturing UI tree on the stack) |

Only `Gameplay` captures the cursor (`captures_cursor()` returns true for `Gameplay` only).

**Transitions.** `App::set_input_focus()` changes the stored variant, acquires or releases the cursor, and clears all input state in both directions — returning to `Gameplay` must not see keys held by a UI consumer; entering UI must not leak gameplay chords. `App::reapply_focus()` re-applies the current variant's cursor state without changing it; called on window-focus restoration so cursor mode survives transient OS focus loss.

**Event gating.** Mouse delta (`device_event`) is only processed when focus is `Gameplay`. Keyboard and mouse-button events honor egui's `consumed` flag when focus is `DevTools` or `Menu`; in `Gameplay` the flag is ignored. `ToggleDebugPanel` punches through the `consumed` gate regardless of focus — it is the chord that opens and closes the panel.

**Pause-menu routing.** Escape from gameplay and gamepad Start emit `nav.menu`. `nav.menu` opens the registered `pauseMenu` only when the modal stack is empty, closes it when it is active, and is ignored while another modal is active. Escape or gamepad B inside a capturing UI tree emit `nav.cancel`. During gameplay, cancel closes an active `pauseMenu` or a frontend submenu; on frontend frames it closes any modal above the frontend menu. Capture puts focus in `Menu`, releases the cursor, and gates player controls without stopping simulation.

**Accessibility-panel input.** `nav.options` — gamepad Select/Back plus the keyboard default F1 — is an engine-owned global input that toggles the accessibility panel (`ui.md` §4.1). Unlike `nav.menu`, it opens over any menu, a text-entry modal included, and over gameplay. The App consumes it ahead of the capture gate, slider nav-capture, and focus dispatch: it never enters the UI dispatch queue, so no tree claims it, and a focused slider whose `capturesNav` names it neither steps nor swallows it. Decided, not yet built: an armed rebind capture (`player_options.md` §6) will be the one exception, receiving it as the input to bind. Pressed while the panel is the active tree, it closes the panel; pressed while the panel is open beneath another tree, it does nothing. It reads ahead of text entry and acts on the press edge only, so a held F1 toggles once in either direction despite OS key repeat. The toggle applies in game logic ahead of the pause-menu toggle and discards the intents already promoted for that frame, so no confirm, click, or direction captured before the toggle activates a control in the tree the toggle revealed or pushed.

**Frames that draw no UI drop UI input.** Booting, Splash, and Loading frames draw no UI, so any UI input pressed on them — the global input and `nav.menu` included — is dropped, never delivered later: queued intents, pending toggles, and buffered gamepad events alike. A buffered gamepad cancel therefore cannot close the first-launch panel on its first frame, and F1 or Select pressed during a load opens nothing when Running begins.

**Console conventions (decided, not yet built).** Where a tree authors none, the focus engine supplies console defaults: focus restores to the opener on return (a pop that reveals the tree again), while a fresh push of a previously visited tree lands on its initial focus; directional nav gets an engine-default hold-to-repeat, and a held slider step accelerates and clamps at its bounds. Nested focus groups are reachable by directional nav.

**Adding a new focus mode.** Add the variant to `input/focus.rs`, update the `captures_cursor` match and its exhaustive-match test, wire `set_input_focus` and `reapply_focus` in `main.rs`.

---

## 6. Gamepad Handling

gilrs provides a unified gamepad API across platforms.

| Concern | Approach |
|---------|----------|
| Dead zones | Per-stick radial dead zone. Inputs below the threshold read as zero. Configurable per player preference. |
| Look options | Decided, not yet built: gamepad look sensitivity, look-stick dead zone, and invert-Y are player options separate from mouse look and the move stick. Sprint gains hold/toggle modes on the crouch-mode pattern. |
| Triggers | Analog axis in [0, 1]. Map to axis actions (e.g., analog acceleration) or threshold to button actions. |
| Action parity | Gamepad bindings map to the same actions as keyboard/mouse. Switching input device mid-play requires no mode change. |

Gamepad and keyboard/mouse bindings coexist. If both are active in the same frame, binding resolution (section 2) applies.

---

## 7. Subsystem Boundary

The input subsystem produces one thing: an action-state snapshot per frame. Game logic is its only consumer.

| Boundary rule | Rationale |
|---------------|-----------|
| Snapshot is the only output | Game logic depends on action semantics, not input hardware |
| App composition writes cross-subsystem state | Store slots driven by input observation (e.g. `input.mode`) are written by App-side code in the input phase, not by the subsystem — the subsystem's output stays the snapshot |
| UI dispatch precedes action mapping | Events a capturing UI tree consumes ride the `input/ui_dispatch.rs` queue (kinded intents) and reach game logic no earlier than the next frame; all intent sources, including gamepad and assistive-technology actions (`ui.md` §4.2), must enqueue before the frame's `take_ready`/`advance_frame` pair. Decided, not yet built: nav intents resolve from the rebindable binding table rather than fixed keys and buttons; UI dispatch still runs first. |
| No wgpu dependency | Input has no rendering concern. Keeps the module testable without a GPU context. |
| No reverse dependency | Game logic never pushes state back into input mid-frame. Information flows one direction. |
| Configurable bindings are input's concern | Game logic does not know which key maps to which action |

---

## 8. Diagnostic Inputs

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

## 9. Non-Goals

- Motion controls (accelerometer, gyroscope)
- Touch input
- Input recording and replay
- Networked input (prediction, rollback)
- VR/AR input (head tracking, hand controllers)
- Steam Input API integration
