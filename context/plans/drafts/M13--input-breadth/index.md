# M13 Goal F — Input Breadth

## Goal

Make the UI operable: hit-testing, a single-focus model with gamepad-first
navigation, hold-to-repeat, pointer↔focus input-mode switching, the modal UI
stack, and the first interactive widgets (`button`, `slider`, `bar`) — filling
the input-dispatch seam Goal A locked, including the `setState` slot-write
reaction sliders require. Realizes `ui-layer.md` §11–§12, §15 (focus model),
§16.

## Scope

### In scope

- Nav-intent vocabulary (closed Rust enum, wire names per `ui-layer.md` §16)
  carried as the `UiIntent` payload through the existing N→N+1 queue.
- Modal stack: snapshot + retained tree go from one gameplay tree to a stack;
  top tree's capture mode wins; `InputFocus::Menu` gets its consumer; E's
  PushTree/PopTree commands operate it.
- Hit-test surface: flat z-ordered device-pixel rect list with stable node ids
  exported at draw-data build; pointer hover/click resolution.
- Focus engine: `focus: "linear" | "spatial" | { … }` container policy, `wrap`,
  `initialFocus`, `restoreOnReturn`, `focusNeighbors`, engine-drawn
  theme-tokened focus ring.
- Hold-to-repeat on nav intents per container policy
  (`{ initialDelayMs, intervalMs }`); confirm/cancel never repeat.
- Input-mode switching: engine-owned `input.mode` enum slot
  (`pointer` / `focus`); OS cursor shown in pointer mode while a capturing
  tree is on the stack.
- Widgets: `button` (focusable, `onPress` fires a named reaction), `slider`
  (`capturesNav`, value display + `setState` write-back), `bar` (numeric bind
  + `max` → fill fraction; E styleRanges and TW tween apply).
- `setState` reaction primitive: writes a writable slot; readonly targets warn
  and no-op — writability, not ownership, gates the write; engine-owned
  writable slots are valid targets.
- Gamepad: D-pad/stick → nav intents through the same focus path; existing
  dead zone reused.

### Out of scope

- Text entry in any form (`input` widget, IME, caret) — see
  `drafts/M13--text-entry/` (on-screen keyboard + hardware-key routing).
- A drawn (engine-rendered) cursor — OS cursor suffices; revisit with G1.
- `list` and `viewport` widgets; scrolling.
- Mouse-driven slider drag (pointer click sets focus; adjustment is
  nav-intent-driven in v1 — pointer drag is additive later).
- Remappable UI bindings (UI nav reads fixed actions; remapping is the
  existing action-map layer's concern).
- Screen-reader / a11y consumption (G2).
- Multi-value text templates (`"{}/{max}"`, deferred from Goal C) stay
  deferred — `bar` computes its fraction internally, no template needed.

## Acceptance criteria

- [ ] With a capturing tree on the stack: cursor releases, gameplay input
  freezes, and a UI event consumed on frame N reaches game logic no earlier
  than frame N+1 (extends A's existing ordering test to real intents).
- [ ] Pushing a second tree freezes the one below (no focus, no activation);
  popping restores the lower tree's focus when it declared
  `restoreOnReturn`; HUD (passthrough tree) never captures.
- [ ] D-pad/keyboard moves focus through a `vstack` in tree order
  (`"linear"`), wraps when `wrap: true`, and moves nearest-neighbor by
  direction in a `grid` (`"spatial"`); `focusNeighbors` overrides win;
  `initialFocus` selects the starting node; the focus ring draws around the
  focused node's rect using theme tokens.
- [ ] Holding a nav direction repeats at the container's declared
  delay/interval in dt-accumulated time; releasing stops; confirm/cancel
  fire once per press regardless of hold, absent a per-button
  `repeatOnHold` opt-in (added by the text-entry plan).
- [ ] Mouse motion sets `input.mode` to `pointer` (cursor visible, ring
  hidden); any stick/D-pad/nav-key input sets `focus` (cursor hidden, ring
  visible); a widget bound to `input.mode` toggles visibility accordingly;
  mode is inert while no capturing tree is on the stack.
- [ ] Pointer hover + click on a `button` and gamepad confirm on a focused
  `button` both fire its `onPress` named reaction through the reaction
  registry — identical observable effect.
- [ ] A focused `slider` consumes left/right nav (`capturesNav`), steps its
  value by `step` within `[min, max]`, and writes the bound writable slot via
  `setState` on the N+1 frame; `setState` against a readonly slot warns and
  leaves the value unchanged.
- [ ] A `bar` bound to `player.health` with `max: 100` renders fill fraction
  `value/max` clamped to [0, 1]; styleRanges (E) recolor its fill; a TW tween
  on the bind eases the displayed fraction.
- [ ] Focus traversal, hit-testing, repeat timing, and stack capture are
  covered by CPU-side tests (layout-tree / draw-list assertions, no GPU
  goldens); descriptors without new fields keep pre-F wire form
  byte-identical.
- [ ] Typedefs + SDK surface in both runtimes; `docs/scripting-reference.md`
  covers nav intents, focus props, the new widgets, and `setState`.

## Tasks

### Task 1: nav-intent vocabulary + dispatch payload

Closed Rust `NavIntent` enum (`Up Down Left Right Next Prev Confirm Cancel
Menu Options`) with wire names `"nav.up"` … `"nav.options"`; input stage maps
actions (keyboard arrows/enter/escape, D-pad, stick-past-deadzone edge,
mouse click/position) to intents and enqueues them as the `UiIntent` payload
in `input/ui_dispatch.rs` (replacing the opaque seq-only marker; keep `seq`).
Track cursor position from winit `CursorMoved` while the cursor is released.
TS template-literal type + Luau string union emitted in typedefs.

### Task 2: modal stack

`UiReadSnapshot.gameplay_tree: Option<AnchoredTree>` →
`trees: Vec<UiTreeEntry>` (descriptor + capture mode + name); retained side
becomes a Vec of per-tree retained state with per-tree dirty gating; draw
bottom→top. App-side stack owner consumes E's PushTree/PopTree commands and
named-tree registrations; engine push/pop API for pause/dialog. Top tree's
`UiCaptureMode` drives `UiDispatch::set_mode` and `InputFocus::Menu`
(cursor release). Splash becomes the degenerate one-entry stack.

### Task 3: hit-test + focus engine

Draw-data build exports a flat list `(node_id, device_rect, z)` for
focusable/interactive nodes (stable string `id` field added to widget
descriptors, auto-generated when absent — runtime-only, never serialized,
preserving the byte-identical round-trip; regenerated deterministically
from tree position on rebuild; focus restore across structural rebuilds
relies on authored ids). Focus engine (CPU, renderer-local
state keyed per tree): policies `linear` (tree order, wrap) and `spatial`
(nearest node center in the directional half-plane), `focusNeighbors`
override, `initialFocus`, `restoreOnReturn`. Pointer hits resolve by topmost
z. Focus ring drawn by the UI pass around the focused rect (theme-token color
+ spacing inset). Hold-to-repeat timer per container policy, dt-clocked.

### Task 4: `setState` + interactive widgets

`setState` system-reaction primitive (E's command path): `{ slot, value }`
applied to writable modder-declared slots at the game-logic stage, readonly
rejected with a warning. Widgets: `button { id, label, onPress }` — focusable,
activation fires the named reaction; `slider { id, label, bind, min, max,
step, capturesNav }` — consumed nav steps the value and emits `setState`;
the slider renders its current numeric value as text beside its label;
`bar { bind, max, fill, background, styleRanges? }` — fill fraction from
bound value, calls E's styleRanges evaluator when present, tween-compatible;
`bind` follows the `PanelBind`/`TextBind` shape precedent (slot name +
optional tween). All additive to the
`Widget` union, camelCase, round-trip-stable.

### Task 5: input-mode slot + gamepad polish + demo

Engine-owned `input.mode` enum slot (`pointer` / `focus`) written by the
input stage on mode transitions (mouse motion vs. nav input; small dt
debounce so jitter doesn't flap); OS cursor visibility follows it while a
capturing tree is on the stack. Gamepad stick-as-D-pad edge detection for nav
(press past deadzone = one intent, repeat handled by Task 3's timer). Demo: a pause-style menu tree (buttons + an `audio.master`-bound slider)
pushed onto the stack, fully gamepad-navigable; HUD `bar` bound to the health
proxy. `audio.master` does not exist in engine code — the demo mod declares it
via `defineStore` (writable), and Task 5 wires an App-side consumer that
applies its value to the audio master bus volume on change, making the slider
audible.

## Sequencing

**Phase 1 (concurrent):** Task 1 (input side), Task 2 (snapshot/stack) —
disjoint files.
**Phase 2 (sequential):** Task 3 — consumes Task 1's intents and Task 2's
per-tree retained state.
**Phase 3 (sequential):** Task 4 — consumes Task 3's focus/activation;
`setState` consumes E Task 2's command path (cross-plan: run after it).
**Phase 4 (sequential):** Task 5 — consumes everything; demo exercises E's
styleRanges if landed.
**Cross-plan:** E and F both edit `descriptor.rs` / `tree.rs` — don't run
E Task 1 and F Task 4 in the same orchestration phase.

## Rough sketch

- Intents/mode: `input/ui_dispatch.rs` (payload), `input/ui_nav.rs` (new —
  action→intent mapping, repeat timer, mode detection).
- Stack: `render/ui/mod.rs` snapshot types; app-side owner near the existing
  snapshot publisher.
- Focus/hit-test: `render/ui/focus.rs` (new), rect export from `tree.rs`.
- Widgets: `render/ui/descriptor.rs` + `tree.rs`; `tree.rs` is large — if it
  passes ~800-line cohesion judgment poorly at implementation time, split
  widget draw-build into `tree/widgets.rs` first (split-before-extend).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| nav intent | `NavIntent::Up` … | `"nav.up"` … | `"nav.up"` literal union | string |
| focus policy | `FocusPolicy` | `"focus": "linear" \| { … }` | `focus` | `focus` |
| repeat | `RepeatPolicy { initial_delay_ms, interval_ms }` | `"repeat": { "initialDelayMs", "intervalMs" }` | same | same |
| neighbors | `focus_neighbors` | `"focusNeighbors"` | `focusNeighbors` | `focusNeighbors` |
| node id | `id: Option<String>` | `"id"` | `id` | `id` |
| button press | named reaction ref | `"onPress": "<reaction>"` | `onPress` | `onPress` |
| nav capture | `captures_nav` | `"capturesNav": ["nav.left", …]` | `capturesNav` | `capturesNav` |
| widgets | `Widget::{Button, Slider, Bar}` | `"button"` / `"slider"` / `"bar"` | constructors | constructors |
| state write | `SystemReactionCommand::SetState` | reaction `"setState"` | `setState` | `setState` |
| bar bind | `PanelBind`/`TextBind` shape | `"bind"` (slot name + optional tween) | `bind` | `bind` |
| label | `label: String` | `"label"` | `label` | `label` |
| slider min/max/step | `min, max, step: f32` | `"min"` / `"max"` / `"step"` | `min` / `max` / `step` | same |
| slider bind | slot name + optional tween | `"bind"` | `bind` | `bind` |
| bar max/fill/background | `max, fill, background` | `"max"` / `"fill"` / `"background"` | `max` / `fill` / `background` | same |
| focus wrap | `wrap: bool` | `"wrap"` | `wrap` | `wrap` |
| initialFocus | `initial_focus: Option<String>` | `"initialFocus"` | `initialFocus` | `initialFocus` |
| restoreOnReturn | `restore_on_return: bool` | `"restoreOnReturn"` | `restoreOnReturn` | `restoreOnReturn` |
| mode slot | n/a | n/a | `input.mode` | `input.mode` |

## Open questions

- Stick-repeat feel (treat held stick like held D-pad vs. analog-scaled
  rate) — ship D-pad-equivalent; tune later.
- Whether `bar` needs a vertical direction in v1 — shipped horizontal-only;
  additive field later.
