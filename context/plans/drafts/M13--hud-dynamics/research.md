# Research — M13 Goal E: HUD dynamics (grounding for /draft-plan)

> **Status:** pre-spec research. No `index.md` exists yet — this file grounds the
> drafting session. It is not a spec; do not implement from it.
> **Read this when:** drafting or reviewing the Goal E spec (`styleRanges` +
> `onStateCrossing` + UI reaction helpers).
> **Wave context:** Goal E and Goal F (`drafts/M13--input-breadth/`) are the next
> M13 wave — two specs, drafted separately, runnable in one `/orchestrate` wave.
> Roadmap: D → (E ‖ F); D and TW are shipped, so both are unblocked. See
> §Cross-spec coordination below for the E/F boundary.

---

## 1. Scope from roadmap + research doc

Roadmap (M13, Goal E): `styleRanges` (renderer-local continuous value → style,
`ui-layer.md` §10) + `onStateCrossing` (discrete crossings firing reaction lists,
`ui-layer.md` §11), reusing the existing entity reaction registry. Absorbs the UI
reaction helpers — `flashScreen` / `playSound` / `rumble` / `showDialog` /
`openMenu` (`ui-layer.md` §15). After D: `styleRanges` resolve theme tokens D
defines. Concurrent with F.

Design intent (`context/research/ui-layer.md`):

- **§10 styleRanges** — entries ordered by `upTo` (fraction of max), first match
  wins, trailing entry is the default; `color` resolves a theme token; effect
  overlays (`pulse` sinusoidal, `flash` one-shot) are a finite renderer-known set;
  applies to `bar`, `text`, `panel` only.
- **§11 onStateCrossing** — fires when a slot value crosses a threshold
  (e.g. `{ below: 0.20 }`); handler list is pre-registered reactions, no script at
  fire time; crossings computed engine-side after game logic writes; dispatch is
  synchronous; same registry as entity reactions.
- **§15 SDK shape** — `sdk/lib/ui/reactions.{ts,luau}` hosts the helpers.
  `setState` is listed there but its decision is explicitly deferred (see §5).

## 2. What shipped that E builds on

- **C (state binding)** — once-per-frame `UiReadSnapshot` (descriptor tree,
  resolved slot values, `time_seconds`), subscriber-aware diffing →
  relayout/redraw split, engine-owned `player.health` / `player.ammo` proxy
  slots. `plans/done/M13--state-system/` defers `styleRanges` / `onStateCrossing`
  to E by name.
- **D (fonts + theming)** — token table with the semantic required set
  (`critical` / `warning` / `ok` / `panel.default`); `styleRanges` colors resolve
  against it. `plans/done/M13--fonts-theming/` line: "`styleRanges` / value→token
  mapping — E (resolves against this table)."
- **TW (value tweening)** — renderer-local animated display values in the
  retained tree. `plans/done/M13--ui-value-tweening/`: "TW animates *values*; E
  maps values to styles and crossings to reactions. Composition (a tweened
  display value driving a `styleRange`) is E's concern against this shipped
  primitive."
- **M12 audio foundation** — `audio` module with buses incl. a UI bus and a
  `play()` entry point; `playSound` is a thin reaction wrapper over it.

## 3. Key code anchors

Line numbers drift — re-verify when touching a listed file.

**Reaction registry + dispatch** (the surface `onStateCrossing` reuses):

- `crates/postretro/src/scripting/reactions/registry.rs` —
  `ReactionPrimitiveRegistry` (struct ~line 16, `register()` ~25). Handler
  signature: `Fn(&mut EntityRegistry, &[EntityId], &serde_json::Value) ->
  Result<(), ReactionError>`. Existing primitives: `setEmitterRate`,
  `setSpinRate`, `setAnimationState`, `applyDamage`, plus the `setFog*` family.
  **Note:** today's handler signature is entity-targeted; the UI helpers
  (`flashScreen`, `playSound`, `rumble`) are not entity mutations — the spec must
  decide whether to widen the handler context or register UI reactions through a
  parallel arm (see §5).
- `crates/postretro/src/scripting/reaction_dispatch.rs` — `fire_named_event`
  (~112) and `fire_named_event_with_sequences` (~143): named-event → reaction
  resolution over `DataRegistry.reactions` (`Vec<NamedReaction>`,
  `ReactionDescriptor::{Primitive, Sequence, Progress}`). A crossing firing a
  reaction list naturally routes through this path.
- `crates/postretro/src/scripting/data_descriptors.rs` — `NamedReaction` (~64),
  descriptor parsing for both runtimes (~879 TS / ~1969 Luau).

**State store / snapshot** (where crossing detection hooks):

- `crates/postretro/src/scripting/slot_table.rs` — `SlotTable`, `SlotValue`
  (Number/Boolean/String/Enum/Array), `SlotSchema` (readonly, range, persist).
- `crates/postretro/src/scripting/primitives/store.rs` — `read_store_slot`
  (~309), `write_store_slot` (~323, engine-side write path).
- `crates/postretro/src/render/ui/mod.rs` — `UiReadSnapshot` (~222–244),
  `set_ui_snapshot()` published after game logic, before render.
- Crossing detection does **not** exist anywhere. Per `ui-layer.md` §11 it is
  computed engine-side (slot-table side, after game logic writes) — *not* in the
  renderer — because it must fire even when no widget binds the slot. Needs
  per-subscription previous-value state and a registration surface (load-time
  declared, VM-dropped — same pattern as entity reactions).

**Retained tree / style plumbing** (where styleRanges evaluates):

- `crates/postretro/src/render/ui/descriptor.rs` — `Widget` internally-tagged
  enum (~65): `text` / `panel` / `image` / `vstack` / `hstack` / `grid` /
  `spacer`. `TextWidget` (~89: `content`, `fontSize`, `color`, `bind`),
  `PanelWidget` (~162: `fill`, `border`, `bind`). No `styleRanges` field, no
  `bar` widget (bar is **F's**).
- `crates/postretro/src/render/ui/tree.rs` — `UiTree` over taffy; per-node
  `NodeContext` holds TW's tween state — the natural home for per-node
  styleRange/effect state (pulse phase, flash clock). UI time is the snapshot's
  dt-accumulated `time_seconds`, never wall clock.
- `crates/postretro/src/render/ui/theme.rs` — `UiTheme`, `resolve_color` /
  `resolve_spacing` / `resolve_font`; unknown-token degradation (magenta /
  `body` / zero + warning) already defined — styleRanges inherits it.

**Helper backends:**

- `playSound` — `crates/postretro/src/audio/mod.rs`: bus parsing (Sfx/Music/UI)
  and `play()`. Viable today as a thin reaction.
- `rumble` — `crates/postretro/src/input/gamepad.rs`: `GamepadSystem` wraps
  `Gilrs`; **no force-feedback support exists**. gilrs exposes ff via its
  `gilrs::ff` effect API — verify the exact API shape (effect builder vs.
  simple set-vibration) against the gilrs version in `Cargo.toml` during
  drafting; needs active-gamepad lookup + duration timeout tracking.
- `flashScreen` — per `ui-layer.md` §13: reaction sets a UI-owned slot
  (e.g. `screen.flash`); a full-screen quad widget reads it. **SE (post-UI
  screen effects, a later wave) consumes the same slots — design the slot
  surface with SE in mind.**
- `showDialog` / `openMenu` — push onto the **modal UI stack, which is F's
  deliverable**. See §Cross-spec coordination.

## 4. Contract constraints the spec must honor

From `context/lib/ui.md`:

- No store write ever originates in the UI module; styleRange/effect state is
  renderer-local presentation state.
- styleRanges evaluate the value the widget renders — which is the **display
  value** when a tween is active (TW contract: the widget never renders the slot
  directly while easing). Crossings, by contrast, watch the **authoritative**
  slot. State both facts explicitly in the spec.
- UI time is dt-accumulated game time (snapshot `time_seconds`); pausing game
  logic pauses pulse/flash effects.
- Renderer never reads the live slot table — anything renderer-side reads the
  snapshot; the crossing detector is engine-side and reads the live table after
  game-logic writes.
- Descriptor wire format is a locked deliverable (B): new fields are additive,
  camelCase, `skip_serializing_if` round-trip-stable; `styleRanges` placement
  (sub-object of `bind` vs. sibling field) is a wire-format decision the spec
  must pin.

## 5. Open questions for the draft session

1. **Registry arm for non-entity reactions** — widen
   `ReactionPrimitiveRegistry`'s handler context (currently
   `&mut EntityRegistry`-shaped) or add a parallel UI-reaction arm dispatched by
   the same named-event path? Decide; don't fork the event vocabulary.
2. **`styleRanges` wire placement** — inside the `bind` object (pairs with the
   slot it watches) vs. a sibling widget field. Lean: sibling field referencing
   the existing `bind` — keeps `bind` minimal and lets static (unbound) widgets
   stay untouched — but pin it in the spec.
3. **Effect taxonomy v1** — research names `pulse` + `flash`. Keep the set
   closed and renderer-known; anything more (`fade`, `scale`) needs a consumer.
4. **Crossing threshold semantics** — `{ below: x }` / `{ above: x }`: edge
   direction, equality inclusion, re-arm rules (fire once per crossing, re-fires
   only after re-crossing back). Initial-value behavior on level load (does a
   slot starting below threshold fire?). Must be pinned — this is modder-facing.
5. **`setState`** — C deferred it; F's slider write-back is its first real
   consumer. Recommend E keep it out of scope (E's helpers are all fire-and-
   forget) and let F or G1 own the decision.
6. **`showDialog` / `openMenu` vs. F's modal stack** — see below.

## Cross-spec coordination (E ‖ F in one wave)

- **Shared files:** both specs extend `render/ui/descriptor.rs` and
  `render/ui/tree.rs` (`NodeContext`). Orchestration should sequence the
  descriptor-touching tasks of E and F into different phases, or accept rebase
  cost — same merge-coordination class as the roadmap's old F/D note.
- **`bar` widget:** F ships it; E's styleRanges evaluator must be widget-
  agnostic (value-in → resolved-style-out) so F's bar adopts it without E
  changes. E demonstrates on `text` + `panel`.
- **`showDialog` / `openMenu`:** depend on F's modal stack. Options: (a) defer
  these two helpers to a late phase of the wave sequenced after F's stack task;
  (b) move them into F's spec. Either is fine; don't have E invent a stub stack.
- **SE (later wave)** consumes the `screen.flash`-style slots E defines — name
  them as a published surface, not an internal detail.
