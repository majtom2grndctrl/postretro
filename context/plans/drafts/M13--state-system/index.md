# M13 Goal C — State Binding (the UI decoupling seam)

> Consumes the **Mod State Store** (`../../done/mod-state-store/index.md`, shipped scripting-foundation prereq). Grounding: `research.md` (esp. §9 anchors, §11 decisions).

## Goal

Consume the mod state store in the UI: a once-per-frame published value snapshot, descriptor binding by slot name, subscriber-aware value diffing → relayout/redraw split, the retained `UiTree`, and a static proxy feeding `player.health` / `player.ammo`. This is the decoupling seam — HUD widgets bind to store slots like `player.health`; a static proxy feeds those slots today, real game logic feeds them later, with no code dependency either direction. The third spec of Milestone 13 (spine A → B → C, sequential; A and B shipped).

C also **publishes the engine-owned `player.*` slot schema** (declared in the store) as the contract Milestone 10's entity health/damage task honors.

## Scope

### In scope

- **Published read handle.** Extend `UiReadSnapshot` to carry a resolved slot-value snapshot, captured once per frame after game logic by `App`, alongside B's `gameplay_tree`. The snapshot **clones** resolved values — the renderer never touches the live store table.
- **Descriptor `bind`.** Optional binding on `text` (content ← slot, with a single-`{}` format template) and `panel` (fill ← color/array slot). String slot-name reference; literal content/fill remains the unbound path.
- **Retained `UiTree` + diffing → relayout/redraw split.** Hold the gameplay `UiTree` on the `Renderer` across frames (B's deferred follow-up). Subscriber-aware diffing: only slots bound in the tree are compared frame-over-frame (store value vs. the node's last resolved value). A layout-affecting change (text content/size) marks the node dirty → taffy relayout; an appearance-only change (color/fill) refreshes the draw list from cached layout with **no** relayout. The draw list re-reads live slot values every frame under cached layout.
- **Static proxy.** Engine-side stand-in (in `App`) that writes `player.health` / `player.ammo` and animates `intro.flashColor` each frame through the store's engine-side `write_store_slot` accessor (bypasses `readonly`, so it can write the readonly `player.*` slots; the script-facing `storeWrite` would reject them). Load-bearing — the real producer (M10 entity health) does not exist yet.
- **Publish the `player.*` schema as the M10 contract.** The engine-owned `player.health` / `player.ammo` slots (registered via the store) are the typed, readonly contract (engine-owned, no range/default) M10's health/damage task writes.
- **Demo screen + CPU test gate.** A Rust-built descriptor binding `player.health` / `player.ammo` to `text` and `intro.flashColor` to a `panel` fill; a reference demo mod (TS + Luau) declaring `intro` via `defineStore`. CPU draw-list / diff / recompute-counter assertions are the hard gate.

### Out of scope

- **The store mechanism** — the slot table, `defineStore`, schema validation, ownership, persistence, the branded `StateValue<T>`, the read/write API. → **Mod State Store** (prereq spec).
- **`styleRanges` / `onStateCrossing`** — value→style maps and discrete crossings → reactions. → **E**.
- **Value tweening / eased display values** — animating a value toward a target over time. → **TW**. The proxy toggles/sets values directly; it does not ease.
- **The `bar` widget** — the HUD health bar. → **F**. C's demo uses `panel` + `text` only.
- **Component-local state** — `liveValue()`, per-component ephemeral cells. → **G1**.
- **Mod-facing UI-reaction `setState`** — writing a slot from a UI event/reaction. → **E / F**. C's writes are engine-side (proxy + store engine API).
- **Script-authored descriptors / SDK factory sugar / handle ergonomics** — `from_*_value` ingestion, JSX/factory, `audio.master.get()/.set()`, `sdk/lib/ui/`. → **G1**. The descriptor stays Rust-built (B precedent); `bind` resolves by string slot-name.
- **Theme tokens, multi-font** → D. **Input** → F. **Screen-space effects, egui retirement** → SE / BIS.
- **Multi-value text templates** (`"{}/{max}"`) — single-`{}` only in C; multi-value lands with `bar` (F).

## Acceptance criteria

- [ ] `UiReadSnapshot` carries a resolved slot-value snapshot captured once per frame after game logic; the renderer reads slot values only from the snapshot, never the live store table.
- [ ] A `text` node bound to `player.health` renders the slot's current value through its format template; a `panel` whose `fill` is bound to `intro.flashColor` renders the slot's color.
- [ ] The static proxy populates `player.health` / `player.ammo` and updates `intro.flashColor` each frame through the store's engine write API.
- [ ] The gameplay `UiTree` is retained on the renderer across frames. An appearance-only bound change (the color flash) refreshes the draw list **without** a taffy relayout — the tree's recompute counter does not increment. A layout-affecting bound change (text content that re-measures) **does** trigger relayout — the counter increments.
- [ ] Value diffing is subscriber-aware: a slot with no binding in the tree changing value invalidates nothing (no draw-list rebuild, no relayout).
- [ ] After the flash settles to a constant color, a no-change frame performs no draw-list rebuild and no relayout (the dirty-gate short-circuits in production — the B follow-up is closed).
- [ ] The engine-owned `player.health` / `player.ammo` slots are exposed to the bind path as a typed, readonly schema (engine-owned, no range/default) — the published M10 contract.
- [ ] The demo renders `player.health` / `player.ammo` as text and a subtle same-hue `panel` flash for ~3 s, then solid. Verification reuses A/B's approach: pure-CPU draw-list / diff / recompute-counter assertions plus a manual run per the project build/run commands — no new golden image.

## Tasks

### Task 1: Published read handle
Extend `UiReadSnapshot` (`render/ui/mod.rs`) with a resolved slot-value snapshot field. `App` builds it from the store's slot table after game logic and before render, through the existing `set_ui_snapshot` path. The snapshot clones resolved values so the renderer never touches the live table — preserves the renderer/game-logic boundary. `App` iterates the slot table (`SlotTable::iter`) to build it; engine reads of individual slots use `read_store_slot`. Depends on the store (slot table + read accessors).

### Task 2: Static proxy + `player.*` schema publication
The engine registers `player.health` / `player.ammo` as readonly engine-owned slots (via the store's API) — the published M10 contract. An engine-side proxy (owned by `App`, holding its `ScriptCtx` clone) each frame sets `player.health` / `player.ammo` to fixed demo values and computes `intro.flashColor` from a level-load timer: toggle between two same-hue RGBA endpoints every 500 ms for 3000 ms, then hold the solid endpoint. `intro` is declared by the demo mod (Task 5); the proxy writes it engine-side (mod writes deferred). The `intro.flashColor` write requires the demo mod loaded — absent it, the proxy skips that write with a `log::warn!`; the `player.*` writes stand alone. Phase 1 cannot exercise the `intro` write until Task 5 lands. Depends on the store.

### Task 3: Descriptor `bind` + value resolution
Add an optional `bind` to `TextWidget` (slot name + optional single-`{}` format template; resolves to `content`) and `PanelWidget` (slot name resolving to `fill`, reading an array/color slot). Wire camelCase per the Boundary Inventory. At draw-data build, resolve bound fields from the snapshot's slot values (number/bool/string/enum → formatted text; array → `[f32; 4]` fill). The color slot is an `Array` of exactly 4 `f32`, linear `[r, g, b, a]` in `0.0–1.0`; a non-length-4 array resolves to the unbound fallback `fill` with a `log::warn!`. A bound `text` with `format: None` renders the resolved value's default string form (no template). Thread the snapshot's slot values into the `layout_tree` / `build_draw_data` path (today `record_splash_ui` and the gameplay path call `self.ui.layout_tree(&tree, viewport, &image_sizes)` — widen the shared signature to take the slot snapshot; the splash call site passes an empty snapshot, behavior unchanged). Depends on the store (slot table + slot-value enum) + Task 1. C binds by string and reads the slot-value enum from the snapshot; it does not consume `StateValue<T>` (store/G1 layer).

### Task 4: Retained `UiTree` + subscriber-aware diffing → relayout/redraw split
Hold the gameplay `UiTree` on the `Renderer` (today `layout_tree` builds a fresh `UiTree::from_descriptor` every frame — `render/ui/mod.rs`). Rebuild only on descriptor structural change or viewport resize; otherwise reuse. Record, per bound node (in the tree's `NodeContext`), which slot it binds and that field's last resolved value. Each frame, diff only bound slots (subscriber-aware) against the new snapshot: a layout-affecting change (text content → re-measure) marks the node dirty so taffy relayouts; an appearance-only change (panel fill / text color) refreshes the draw list with no relayout. Split `build_draw_data` so layout-compute stays gated (the `viewport_changed || structural_change` condition extended with the value-driven dirty mark) while draw-data collection runs each frame from cached layout reading live snapshot values. The recompute counter must not increment on an appearance-only frame. Depends on Tasks 1 + 3.

### Task 5: Demo screen + CPU test gate
A Rust `build_demo_descriptor` (B's `build_splash_descriptor` precedent) binding `player.health` / `player.ammo` to `text` and `intro.flashColor` to a `panel` `fill`. A reference demo mod in TS and Luau declaring `intro` via `defineStore`. Extend A/B's CPU assertion harness: bind resolution, subscriber-aware diff, the appearance-only-no-relayout vs content-change-relayout split, the post-settle no-recompute frame. Splash behavior and egui untouched (the splash call site only forwards an empty snapshot through the widened `layout_tree` signature). Depends on Tasks 2, 3, 4.

## Sequencing

**Prereq:** the Mod State Store spec ships first (slot table, `defineStore`, engine-owned slots, read/write, persistence).

**Phase 1 (concurrent):** Task 1 (read handle), Task 2 (proxy + schema) — each depends only on the store, independent of each other.
**Phase 2 (sequential):** Task 3 — `bind` + resolution. Consumes Task 1's snapshot.
**Phase 3 (sequential):** Task 4 — retained tree + diffing. Consumes Task 3.
**Phase 4 (sequential):** Task 5 — demo + test gate. Consumes Tasks 2, 3, 4.

## Rough sketch

**Read handle + resolution.** `UiReadSnapshot` gains a resolved-values map (cloned slot name → value). `App` fills it after game logic from the store table. `bind` on a widget is a slot-name string; resolution happens at draw-data build, reading the snapshot. No `StateValue` handle / named-leaf IR in C — that ergonomic layer is G1; C binds by string. Binding by slot name keeps C **name-stable under the entity-model refactor**: `player.health` projects whatever authoritative producer the store exposes — engine health component today, a generic scalar-stat or a future representation later — with no change to C. Write the demo and the M10 contract against the stable surface (slot names, `defineStore`), never against entity-component internals.

**Retained tree + split.** Hold `Option<UiTree>` (plus the descriptor it was built from) on the `Renderer` for the gameplay path. The tree's `NodeContext` carries the binding (slot name + target field) and last resolved value per bound node. Layout-compute stays behind the gate (`viewport_changed || structural_change || value_forced_dirty`); draw-data collection runs every frame from cached taffy rects, substituting current bound values. A color/fill change touches only draw data; a text-content change forwards to taffy's `TaffyTree::mark_dirty(node)` (net-new — `UiTree` exposes no such method today; the current gate only *queries* `taffy.dirty(root)`) so the gate fires. The splash path keeps rebuilding its descriptor each frame (transient, pre-state) — only the gameplay path retains.

**Bind shape.** Per-widget optional `bind` sub-struct: `TextBind { slot, format }` (single-`{}`), `PanelBind { slot }` (array/color slot → `fill`). Exact field set is the implementer's call within the casing rules below.

**Key files.** `render/ui/mod.rs` (`UiReadSnapshot`, `layout_tree`), `render/ui/descriptor.rs` (`bind`), `render/ui/tree.rs` (`UiTree`, `NodeContext`, gate, draw-data split), `render/mod.rs` (`ui_snapshot`, `set_ui_snapshot`, gameplay path, retained tree, proxy), new `render/ui/demo.rs` (`build_demo_descriptor`), `content/dev/scripts/` (reference demo mod). The store provides the slot table + engine-side read/write accessors (`read_store_slot` / `write_store_slot`).

## Boundary inventory

The descriptor `bind` crosses Rust ↔ wire ↔ JS/TS ↔ Luau (the `defineStore` schema casing is pinned in the store spec). Rust snake_case; wire/JS/Luau camelCase.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| text bind | `Option<TextBind>` (`slot`, `format`) | `bind` (`{ slot, format }`) | `bind` | `bind` |
| panel fill bind | `Option<PanelBind>` (`slot`) | `bind` (`{ slot }`) | `bind` | `bind` |
| slot reference | `String` | dotted, `"player.health"` | `"player.health"` | `"player.health"` |
| format template | `Option<String>` | `format` (single `{}`) | `format` | `format` |

## Open questions

- **`UiReadSnapshot` value carrier.** The snapshot clones resolved values (decided — renderer never touches the live table). Residual: clone the full slot set vs. only bound slots. Recommend bound-only (subscriber-aware) once Task 4's binding inventory exists; full set is simpler for Task 1. Confirm.
- **Previous-frame value ownership.** The per-node last-resolved value lives in the tree's `NodeContext` (C owns diffing). Confirm vs. a parallel side-table on the `Renderer`.
- **Format scope.** Single-`{}` template in C; multi-value (`"{}/{max}"`) lands with `bar` (F). Confirm.
- **Demo mod necessity.** C proves the store's `defineStore` via a reference mod declaring `intro` (the descriptor stays Rust-built until G1). Confirm the split — declare-in-script, bind-in-Rust — reads cleanly as the C deliverable.
