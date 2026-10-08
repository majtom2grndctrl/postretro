# Input

> **Read this when:** building or modifying the input subsystem, adding new player actions, changing how game logic consumes input, or routing UI navigation, dropping UI input on frames that draw no UI, or rebinding.
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

A single action can have multiple physical bindings. W key and left stick Y both map to the forward/back movement axis. Bindings are data, not code.

### Commands, activators, and layering

- **Commands.** Bindings name commands by stable snake_case ID: gameplay commands from the closed action set, UI commands from the nav intents plus the tab commands (`nav_tab_next`, `nav_tab_prev`) and the on-screen keyboard shortcuts (`text_backspace`, `text_space`, `text_commit`). An axis action splits into digital per-direction commands (`move_forward`) and an analog command (`look_x`); a digital command accepts keys, buttons, and half-axis stick inputs (`left_stick_up`), and a half-axis input on movement keeps its stick magnitude. An analog command accepts axes only, so a stick swap is an ordinary rebind. Engine IDs never contain `.`; `<mod_id>.<name>` is reserved for mod-defined commands, split at the last `.`. Saved data and manifests never use enum indices.
- **Inputs by physical position.** Keyboard keys use W3C `KeyboardEvent.code` names, mouse inputs fixed snake_case names, and gamepad buttons and axes position names (`south`, `left_shoulder`). Every name the engine writes parses back to the same input. Analog polarity is physical and fixed per source: every source reads + for right and up (`mouse_y` is negated, since the OS reports down as +), and `look_x` / `look_y` read + as look right / look up. Labels and glyphs resolve at display time.
- **Layering.** The effective binding is the player override, else the author default from the manifest `input` block, else the engine default (`player_options.md` §6); no block means the engine table. The confirm/cancel swap then exchanges the gamepad bindings of `nav_confirm` and `nav_cancel`; it is a player option, never automatic by device family. The table rebuilds at mod init, on hot reload, on rebind, on a swap change, and when co-op host tuning installs or clears. A pending resolution on a key whose bindings changed is cancelled, and a held key the rebuild newly binds waits for a fresh press.
- **Author validation.** The `input` block degrades per command and device class: an unknown command or input, an ill-fitting input (a key on an analog command, an axis on a digital one), a refused activator, a bad threshold, or a guard violation is diagnosed, and that command falls back to its engine default on that class alone. A fallback that would collide leaves the command unbound there. Of two valid author entries that conflict, the later one in manifest order is unbound; Luau tables carry no key order, so the Luau drain orders commands by ID. Hot reload re-validates.
- **Relevance.** Which commands a game uses is derived from its descriptors by an exhaustive match, so an unclassified command fails to compile: dash and crouch from movement descriptors, reload from a magazine weapon, alt-fire from a secondary activation. While participating in co-op, relevance also follows the installed host tuning, since tuning sites keep no local fallback. An irrelevant command is unbound, unlisted, never part of a conflict, and draws no glyph. The author's manifest can force a gameplay command shown or hidden. UI commands are always relevant and cannot be hidden. The fly-cam commands `move_up` and `move_down` are dev-only: bound to their engine defaults, unlisted, and never part of a conflict.
- **Activators.** Each binding carries an activator: `press`, `release`, `tap` (max time), or `hold` (min time). Authors set them; players rebind keys only, and each input's activator is derived (`player_options.md` §6). Each command accepts a fixed set: shoot and alt-fire `press` only, because charge requires it; analog and wheel-notch commands `press`; sprint, crouch, and the movement commands `press` or `hold`, since movement reads a level that a tap or release would only pulse; every other command any kind. Activators resolve client-side into the Pressed → Held → Released phases commands already read, from buffered key and gamepad edges, so a press and release between two frames still resolves and an edge survives a frame with no tick; the wire carries resolved intent and gains nothing. Hold/toggle modes (`crouch_mode`, `sprint_mode`) apply on top of the resolved phases. Thresholds default to 0.2 s and are scaled by `hold_timing_scale` (`player_options.md` §5); a key keeps the thresholds it went down with. Losing focus or a capturing tree opening cancels a pending resolution, and neither binding fires; a key held across it acts again only on its next press.
- **Shared keys (Steam rule).** Within a context, a key carries at most one short binding (`press`, `release`, or `tap`) plus one `hold`. On a shared key, the hold fires at its min and the short binding never does. A release before the min fires the short binding, within the tap's max for a `tap`. A release between a tap's max and a hold's min fires nothing. Press-only commands never share a key.
- **Contexts.** Gameplay and UI commands are separate sets, and conflicts are checked only among commands live in the same context: South may be both jump and confirm. UI commands resolve by the tree on top: with no capturing tree only `nav_menu` is live; under a capturing tree the nav commands are, `nav_menu` included, and an input bound to both menu and cancel acts as cancel, so Escape opens the pause menu from gameplay and cancels inside a menu without a conflict. Under a text-entry tree, keyboard keys type their characters and never resolve a UI command; gamepad inputs resolve nav and the text shortcuts, and `nav_menu` is not live.

**Button signal width.** Each consumer chooses its own signal width when reading a button action from the snapshot. `is_active()` (Pressed|Held) is a level signal — it fires on every qualifying tick while the button is held. `ButtonState::Pressed` alone is a rising edge — it fires only on the first tick. Use a rising edge when a held input would wrongly re-trigger each qualifying tick: dash uses `ButtonState::Pressed` because a held dash would re-fire every cooldown-ready tick. Jump uses the level signal (`is_active()`) because the movement system self-gates it via a ceiling rule.

**Weapon edges.** `Shoot` and `AltFire` feed primary and secondary actions. Render-rate capture latches both press and release until a real fixed command consumes them, including a press/release pair between ticks. Each initiation names its client tick and lane; release/cancel names that initiation. Held restarts receive fresh requests from the local controller, never from host-synthesized input. Synthetic held/neutral commands carry no activation edges (`networking.md` §Combat authority).

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

Gameplay-input suspension, including focus loss and capturing menus, latches an explicit activation cancel and clears gameplay press/release latches. A connected client sends that cancellation reliably even on a frame with no fixed tick. Sending intent does not advance weapon execution; the host applies it on its next simulation tick. Neutral input is never a charge release. Local charge feedback clears immediately on suspension.

**Event gating.** Mouse delta (`device_event`) is only processed when focus is `Gameplay`. Keyboard and mouse-button events honor egui's `consumed` flag when focus is `DevTools` or `Menu`; in `Gameplay` the flag is ignored. `ToggleDebugPanel` punches through the `consumed` gate regardless of focus — it is the chord that opens and closes the panel.

**Pause-menu routing.** By default, Escape from gameplay and gamepad Start emit `nav.menu`. `nav.menu` opens the registered `pauseMenu` only when the modal stack is empty, closes it when it is active, and is ignored while another modal is active. By default, Escape or gamepad B inside a capturing UI tree emit `nav.cancel`. During gameplay, cancel closes an active `pauseMenu` or the accessibility panel, and pops any tree pushed above the pause menu or the frontend root, so a submenu opened from the pause menu returns to it; a tree with neither beneath owns its own cancel policy. On frontend frames it closes any modal above the frontend menu. Capture puts focus in `Menu`, releases the cursor, and gates player controls without stopping simulation.

**No reserved accessibility input.** No engine key or button opens the accessibility panel; menus reach it through `ui.openAccessibility` (`ui.md` §4.1). `nav_options` defaults to gamepad Select/Back and to no keyboard key; it is live only while a capturing tree is on top, emits an ordinary queued `nav.options` intent, and nothing acts on it today. While the panel is the active tree, `nav.cancel` closes it in every UI-drawing boot state.

**Frames that draw no UI drop UI input.** Booting, Splash, and Loading frames draw no UI, so any UI input pressed on them — `nav.menu` included — is dropped, never delivered later: queued intents, the latched menu toggle, and buffered gamepad events alike. A buffered gamepad cancel therefore cannot close the first-launch panel on its first frame.

**Console conventions.** The focus engine supplies console defaults where a tree authors none: restore on return, engine-default hold-to-repeat, accelerating slider repeat, nested-group traversal, and tabs (`ui.md` §4). Input's share: a held direction or confirm repeats until the release of whichever input is bound to it now, so a rebound direction stops on its new key and an arrow no longer bound to it stops nothing. A direction held while a tree is pushed moves nothing until pressed again. The bumpers default to the tab commands (`nav_tab_prev` on `left_shoulder`, `nav_tab_next` on `right_shoulder`; Q and E on keyboard), which activate the adjacent tab in the top tree's tablist and step Next/Prev in a tree with none. Either stick navigates through whichever half-axis inputs the nav directions bind (the left stick and D-pad by default).

**Rebind capture.** While the controls panel's capture prompt is the active tree, every raw input is taken at intake — keys, mouse buttons, wheel notches, pad presses, trigger crossings, stick halves pushed from rest, and mouse travel for an analog slot — and routed nowhere else: no nav intent, menu toggle, text entry, or gameplay read sees it, on the press or its release. Escape, Start, and Select/Back are capturable. A press that resolved before the prompt became the active tree, its OS key repeat, and a stick already off rest when the prompt opened never reach it. The candidate resolves after the frame's activations. There is no time limit (`player_options.md` §6 has the abandon rules).

**On-screen keyboard shortcuts.** While a text-entry tree is on top, gamepad inputs bound to `text_backspace`, `text_space`, and `text_commit` (by default West, North, and Start) activate the keyboard tree's own backspace, space, and done keys by id, without moving focus; a held backspace shortcut repeats as the key does. Hardware keys never act as shortcuts there: they type, and hardware Backspace and Enter edit and commit as before.

**Adding a new focus mode.** Add the variant to `input/focus.rs`, update the `captures_cursor` match and its exhaustive-match test, wire `set_input_focus` and `reapply_focus` in `main.rs`.

---

## 6. Gamepad Handling

gilrs provides a unified gamepad API across platforms.

| Concern | Approach |
|---------|----------|
| Dead zones | Per-stick radial dead zone. Inputs below the threshold read as zero. The stick bound to look takes the player's look dead zone; the other keeps the engine default. |
| Look options | Gamepad look sensitivity (radians per second at full deflection), look dead zone, and invert-Y are player options separate from mouse look and the move stick (`player_options.md` §6). The look dead zone follows whichever stick is bound to look, so a stick swap moves it. Sprint has hold/toggle modes on the crouch-mode pattern, latching on the resolved phases. |
| Triggers | Digital inputs (`left_trigger`, `right_trigger`) that press at half travel and release below it with a small hysteresis band; no command reads an analog trigger. gilrs 0.11 reports triggers as analog button values rather than axes, so the trigger reads the button value with the axis as a fallback. |
| Action parity | Gamepad bindings map to the same actions as keyboard/mouse. Switching input device mid-play requires no mode change. |
| Device family | Glyphs and the controls panel follow the last-used family: keyboard and mouse, or the active pad's family by USB vendor id (Sony → PlayStation, Nintendo → Nintendo, any other → Xbox). Only deliberate input votes — presses, stick crossings past the dead zone, the pointer-mode switch — so drift never changes it, and one family is chosen per frame. The family never swaps confirm and cancel. |
| Active pad | One pad is active at a time; events from any other pad are ignored. With none active, any input claims the role. Otherwise an idle pad takes it only on a button press or a push of at least half travel, so drift or a release cannot steal it. Frames that draw no UI follow the same rule. |
| Inert until released | A switch lifts the outgoing pad's held inputs to neutral and forgets them. Inputs the incoming pad is already holding when it takes the role stay inert until released, except the press that claimed the role, which acts. A pad that reconnects starts from what it holds at that moment. |
| Disconnect | The active pad's disconnect lifts every held pad command to neutral — never a release. Edges still unresolved at that moment are dropped, so nothing the pad held fires a release-bound or tap command. |

Gamepad and keyboard/mouse bindings coexist. If both are active in the same frame, binding resolution (section 2) applies.

---

## 7. Subsystem Boundary

The input subsystem produces one thing: an action-state snapshot per frame. Game logic is its only consumer.

| Boundary rule | Rationale |
|---------------|-----------|
| Snapshot is the only output | Game logic depends on action semantics, not input hardware |
| App composition writes cross-subsystem state | Store slots driven by input observation (e.g. `input.mode`) are written by App-side code in the input phase, not by the subsystem — the subsystem's output stays the snapshot |
| UI dispatch precedes action mapping | Events a capturing UI tree consumes ride the `input/ui_dispatch.rs` queue (kinded intents) and reach game logic no earlier than the next frame; all intent sources, including gamepad and assistive-technology actions (`ui.md` §4.2), must enqueue before the frame's `take_ready`/`advance_frame` pair. Nav intents resolve from the UI command set's effective bindings per UI context (§2), never fixed keys and buttons; keyboard and gamepad intake share one lookup built with the gameplay table. |
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
