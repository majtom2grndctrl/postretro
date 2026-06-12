# Research — M13 Goal F: Input breadth (grounding for /draft-plan)

> **Status:** pre-spec research. No `index.md` exists yet — this file grounds the
> drafting session. It is not a spec; do not implement from it.
> **Read this when:** drafting or reviewing the Goal F spec (hit-testing, focus,
> nav intents, hold-to-repeat, input-mode switching, modal stack, gamepad UI nav,
> button/input activation, `bar` widget).
> **Wave context:** Goal F and Goal E (`drafts/M13--hud-dynamics/`) are the next
> M13 wave — two specs, drafted separately, runnable in one `/orchestrate` wave.
> See §Cross-spec coordination below for the E/F boundary.

---

## 1. Scope from roadmap + research doc

Roadmap (M13, Goal F): hit-testing, single-focus focus ring, template-typed nav
intents, hold-to-repeat, pointer-vs-focus input-mode switching, the modal UI
stack, gamepad via gilrs, button / input activation (`ui-layer.md` §11, §12,
§16) — filling the seam A locked. Depends on B (and the A seam). The TW roadmap
note also assigns the **`bar` widget** here ("it additionally needs F's `bar`
widget").

Design intent (`context/research/ui-layer.md`):

- **§12 input dispatch** — hit-testing over a flat z-ordered rect list with
  widget IDs; single focused widget; focus ring layout-derived; UI is a *stack*
  of trees (top captures, lower freeze); per-tree capture-vs-passthrough;
  gamepad routes through the same focus/activation path.
- **§15 focus model** — container `focus` prop: `"linear"` / `"spatial"` /
  rich object (`wrap`, `initial`, `repeat`, `restoreOnReturn`); per-node
  `focusNeighbors` override; screen-level `initialFocus`; no `focusOrder`
  numbers — tree order is the default.
- **§16 input model** — nav-intent vocabulary (`nav.up` … `nav.cancel`,
  template-literal-typed in TS); `capturesNav` gives widgets first refusal
  (slider eats horizontal nav); hold-to-repeat declared on the container's
  focus policy (`{ initialDelayMs: 350, intervalMs: 90 }`), nav intents only,
  confirm/cancel never repeat; engine-owned `input.mode` state
  (`pointer` ↔ `focus`) that descriptors bind for cursor/focus-ring visibility.

## 2. The A seam — what exists vs. what F fills

`crates/postretro/src/input/ui_dispatch.rs` (shipped in A, line numbers drift):

- `UiCaptureMode` (~41): `Capture` | `Passthrough` (default). Splash currently
  hardcodes `Passthrough`.
- `UiDispatchOutcome` (~54): `Captured` | `Forward`.
- `UiIntent` (~80): **opaque marker with a monotonic `seq: u64` — no vocabulary
  attached.** F replaces/extends this payload with the real intent vocabulary.
- `UiDispatch`: `set_mode()`, `dispatch_event() -> UiDispatchOutcome`,
  `take_ready() -> Vec<UiIntent>`, `advance_frame()`.
- **N→N+1 contract is proven by test** (`captured_event_reaches_game_logic_
  no_earlier_than_next_frame`, ~207–240): captured on frame N → `pending` →
  promoted at `advance_frame()` → game logic sees it via `take_ready()` on
  frame N+1, never same-frame. All F intents ride this queue.

`crates/postretro/src/input/focus.rs` — `InputFocus` enum:
`Gameplay` | `DevTools` | `Menu`. Only `Gameplay` captures the cursor.
**`Menu` is wired but has no consumer** — F's modal stack is the consumer
(menu open → `InputFocus::Menu`, cursor released, UI captures).

## 3. Key code anchors

Line numbers drift — re-verify when touching a listed file.

**Input subsystem** (`context/lib/input.md` is the contract doc):

- `crates/postretro/src/input/mod.rs` — action-mapping layer; game logic reads
  the action-state snapshot, never raw input; level (`is_active`) vs. edge
  (`ButtonState::Pressed`) signal widths.
- `crates/postretro/src/input/gamepad.rs` — `GamepadSystem` over `Gilrs`;
  radial dead zone hardcoded `DEAD_ZONE = 0.15` (~10); trigger threshold 0.5;
  14 buttons + 4 stick axes polled in `update()` (~59–100). **No repeat
  semantics, no UI-specific routing** — F adds D-pad/stick → nav-intent
  mapping with per-container repeat.
- `crates/postretro/src/input/cursor.rs` — OS-level grab/visibility only
  (`capture_cursor` / `release_cursor` / `set_cursor_visible`); raw motion is
  accumulated delta, not position. **No drawn cursor exists**; pointer
  hit-testing in released-cursor mode needs cursor *position* (winit
  `CursorMoved`), which the gameplay path doesn't track — F adds it.

**Layout / retained tree** (hit-test inputs):

- `crates/postretro/src/render/ui/tree.rs` — `UiTree` over `taffy::TaffyTree`;
  per-node taffy `NodeId`; absolute rects are computed during draw-data build.
  **No flat z-ordered rect list with stable IDs is exported** — F adds the
  hit-test/focus surface here.
- `crates/postretro/src/render/ui/layout.rs` — logical 1280×720 → device-pixel
  projection (`device_scale` ~131, `canvas_origin` ~141, independent edge
  snapping ~176). Hit-testing must use the same projection (device-pixel rects)
  so hits match what's drawn. D's changes here were token-resolution adjacent;
  no structural conflict left from the old F/D warning.
- `crates/postretro/src/render/ui/descriptor.rs` — `Widget` enum (~65): 7 kinds
  (`text`/`panel`/`image`/`vstack`/`hstack`/`grid`/`spacer`). **No `button`,
  `bar`, `input`, `list`, or `slider`.** No general per-node `id` field —
  focusable widgets need stable IDs (research uses string ids, e.g.
  `id: "resume"`). Wire-format additions must be additive + camelCase +
  round-trip-stable (B's locked deliverable).

**Modal stack:**

- `crates/postretro/src/render/ui/mod.rs` — `UiReadSnapshot` (~222–244) carries
  `gameplay_tree: Option<AnchoredTree>` (singular) + `slot_values` +
  `time_seconds`; `RetainedGameplayTree` (~380) retains exactly one tree with
  per-tree theme-generation dirty gating. The retained-tree shape generalizes
  to a stack cleanly (Vec of retained trees, per-tree dirty state, painter's
  order bottom→top, top tree's capture mode wins) — but the snapshot field and
  retained-tree bookkeeping are singular today; F changes both additively.
- Who owns the stack: pushes/pops originate engine-side (pause toggle,
  `showDialog` reaction from E) — app/game-logic side owns the stack content;
  the renderer retains and draws it. Mirrors the existing snapshot split.

**Input-mode slot precedent:**

- Engine-owned readonly slots already exist (`player.health` / `player.ammo`
  proxies, C). `input.mode` follows the same pattern: engine-owned, enum-valued
  (`SlotValue::Enum` exists in `slot_table.rs`), written by the input stage on
  mode transitions (mouse motion → `pointer`; stick/D-pad → `focus`), bound by
  descriptors for cursor/focus-ring visibility.

## 4. Contract constraints the spec must honor

- **Frame-order contract (A, hard gate):** any UI-consumed event resolves to
  game logic no earlier than frame N+1, through the existing `UiDispatch`
  queue. F's activations (button press → reaction fire) ride the same path.
- **Renderer owns GPU** — hit-testing/focus logic is CPU-side; the focus-ring
  *draw* lives in the UI pass; nothing in `input/` touches wgpu types.
- **Descriptor-only authoring, no live VM** — focus policy, `capturesNav`,
  `onPress` are descriptor data; activation resolves to *named, pre-registered
  reactions* (E's registry surface), never a script callback.
- **CPU draw-list / layout-tree assertions are the test strategy** (A) — focus
  traversal, hit-testing, repeat timing, and stack capture are all unit-testable
  without GPU goldens. The N→N+1 test pattern is the precedent.
- **UI time** — repeat timers and mode-switch debounce use dt-accumulated game
  time (snapshot `time_seconds` / input-stage dt), not wall clock.

## 5. Open questions for the draft session

1. **Intent vocabulary representation** — Rust enum mirrored to the
   template-literal TS type (`"nav.up"` …)? Where does the engine map raw
   actions → nav intents (input stage, before `UiDispatch` enqueue)? Lean: a
   closed Rust enum with string wire names matching `ui-layer.md` §16.
2. **Hit-test surface shape** — flat `Vec<(id, device_rect, z)>` exported from
   draw-data build vs. querying the retained tree on demand. Lean: flat list
   rebuilt with the draw list (research §12's stated design).
3. **Focus-ring rendering** — engine-drawn decoration around the focused node's
   rect (theme-tokened) vs. a descriptor widget. Lean: engine-drawn; the
   `FocusRing` widget in research §16 is only a visibility binding.
4. **`bar` widget scope** — F ships the widget (bind + max + fill fraction);
   E's styleRanges and TW's tweening both apply to it via their own seams.
   Where does fraction math live (slot value / `max` prop — see the resolved
   note in `done/M13--state-system/research.md` §2: no fixed range on
   `player.health`; bar math was explicitly deferred to F).
5. **`input` (text entry) scope** — full text editing is heavy (IME, glyphon
   caret math). Decide v1: defer text entry entirely, or ship a minimal
   single-line field? Roadmap names "button / input activation" — confirm
   whether `input` can slip to a later goal without blocking G1/BIS (pause
   menu and HUD need buttons + sliders, not text fields).
6. **Slider write-back / `setState`** — a slider adjusting `audio.master`
   needs the deferred `setState` state-write reaction (C deferred; E is keeping
   out of scope). If F ships sliders, F forces the `setState` decision; if F
   defers sliders to G1/BIS, `setState` stays deferred. Decide scope here
   first — it's the biggest scope lever in the spec.
7. **Mode-switch policy details** — debounce/hysteresis (mouse jitter
   shouldn't flap modes), and whether pointer mode exists at all while the
   cursor is captured in gameplay (likely: mode only meaningful when a
   capturing tree is on the stack).

## Cross-spec coordination (E ‖ F in one wave)

- **Shared files:** both specs extend `render/ui/descriptor.rs` and
  `render/ui/tree.rs`. Sequence the descriptor-touching tasks into different
  orchestration phases or accept rebase cost.
- **Activation → reaction:** F's button/`onPress` fires named reactions through
  the dispatch path E touches (`fire_named_event` in
  `scripting/reaction_dispatch.rs`). F consumes E's registry surface; agree on
  the named-event shape before F's activation task runs.
- **`showDialog` / `openMenu`** (E's helpers) push onto F's modal stack —
  sequence after F's stack task, or move them into F's spec.
- **`bar` widget** is F's; E's styleRanges evaluator stays widget-agnostic so
  bar adopts it without E changes. The deferred eased-health-bar flourish (TW
  note in roadmap) becomes possible once F's bar exists — it lands as a BIS
  built-in, not in this wave.
