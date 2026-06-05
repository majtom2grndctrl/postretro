# M13 Goal C — State System (the decoupling seam)

## Goal

Build the UI state substrate: a slot table, the `defineState` scripting primitive, the branded `StateValue<T>` contract, a once-per-frame published value snapshot, and binding-driven draw-data refresh with subscriber-aware value diffing. This is the seam that lets HUD widgets bind to engine state (`player.health`) without a code dependency on game logic — a static proxy feeds the slots today, real game logic feeds them later. The third spec of Milestone 13 (spine A → B → C, sequential; A and B shipped).

This is the decoupling point of the whole milestone: C **publishes the engine-owned slot schema** (`player.health`, `player.ammo` — typed, ranged, readonly) as the contract Milestone 10's entity health/damage task honors, and owns the `persist: true` save wire format.

## Scope

### In scope

- **Slot table + schema.** An engine-global slot table (survives level loads), holding typed slots keyed by dotted name (`player.health`, `intro.flashColor`). Slot value types: number, boolean, string, enum, array (no nested objects — flat surface, per `ui-layer.md` §9). Per-slot metadata: type, default, optional range, `persist`, `readonly`, current value, previous-frame value (for diffing).
- **`defineState` primitive.** A new scripting primitive that ingests a namespaced slot schema and registers its slots. Installs in both runtimes (QuickJS + Luau) via a new `state` domain module. Scoped `DefinitionOnly` (`defineState` is the first real consumer of that scope). Range/finite validation Rust-side (serde can't bound numbers).
- **Engine-owned slot registration.** The engine registers `player.*` (readonly) at init through the same slot-table API, as the published M10 contract. Modders declare their own namespaces via `defineState`. Same table, two callers; `readonly: true` blocks *script* writes, not engine writes.
- **Branded `StateValue<T>`.** The generated TS/Luau typedefs express `StateValue<T>` as a *generic* branded type (the existing brand emitter is non-generic — this closes that gap).
- **Published read handle.** Extend `UiReadSnapshot` to carry a once-per-frame resolved slot-value snapshot, taken after game logic, alongside B's `gameplay_tree`.
- **Descriptor `bind`.** Optional binding on `text` (content ← slot, with a single-`{}` format template) and `panel` (fill ← color/array slot). String slot-name reference; no handle IR. Literal content/fill remains the unbound path.
- **Retained `UiTree` + diffing → relayout/redraw split.** Hold the gameplay `UiTree` on the `Renderer` across frames (B's deferred follow-up). Subscriber-aware value diffing: only slots bound in the tree are compared frame-over-frame. A layout-affecting change (text content/size) marks the node dirty → taffy relayout; an appearance-only change (color/fill) refreshes the draw list from cached layout with **no** relayout. The draw list re-reads live slot values every frame under cached layout.
- **Static proxy.** Engine-side stand-in that writes `player.health` / `player.ammo` and animates `intro.flashColor` each frame. Load-bearing — the real producer (M10 entity health) does not exist yet.
- **Persisted-slot save wire format.** A versioned flat JSON file for `persist: true` slots: load-on-start (persisted values override declared defaults), save-on-shutdown. Type-checked restore; unknown/mismatched entries ignored with a warn.
- **Demo screen + CPU test gate.** A Rust-built gameplay descriptor binding `player.health`/`player.ammo` to `text` and `intro.flashColor` to a `panel` fill; a reference demo mod (TS + Luau) declaring `intro`/`audio` via `defineState` with a parity test. CPU draw-list / diff / gate assertions are the hard gate.

### Out of scope

- **`styleRanges` / `onStateCrossing`** — value→style maps and discrete crossings → reactions. → **E**. C ships the substrate they read.
- **Value tweening / eased display values** — animating a value toward a target over time. → new roadmap goal **TW**. C's proxy toggles/sets values directly; it does not ease.
- **The `bar` widget** — the HUD health bar. → **F** (owner decision, 2026-06). C's demo uses `panel` + `text` only.
- **Mod-facing slot writes (`setState`)** — the reaction/event write path. → **E / F**. C's writes are engine-side (proxy + engine-owned registration). The persist round-trip is exercised via the engine-side write API, not a script write.
- **Script-authored descriptors / SDK factory sugar** — `from_*_value` descriptor ingestion, JSX/factory functions, the `audio.master.get()/.set()` handle ergonomics, the `sdk/lib/ui/` wrappers. → **G1**. C defines the `defineState` primitive and the `StateValue<T>` brand type; the descriptor stays Rust-built (B precedent), and `bind` resolves by string slot-name.
- **Theme tokens, multi-font** → D. **Input** → F. **Screen-space effects, egui retirement** → SE / BIS.
- **Multi-value text templates** (`"{}/{max}"` interpolating multiple slots) — single-`{}` only in C; multi-value lands with `bar` (F).
- **Per-user save directory resolution** (`dirs`-style dependency) — C writes to a single working-directory-relative path; proper per-user dir is a deferred refinement.

## Acceptance criteria

- [ ] `defineState(namespace, schema)` declares a slot namespace from **both** runtimes with parity (TS and Luau equivalents produce the same slots); the slot table holds the declared slots after mod init; a malformed schema or unknown slot type errors (a returned error, not a panic).
- [ ] A `number` slot declared with `range: [min, max]` clamps an out-of-range write to the range and logs a warning; the clamped value is stored.
- [ ] The engine registers `player.health` and `player.ammo` as `readonly` engine-owned slots at init; a script write to a readonly slot is rejected and logged; an engine-side write to the same slot succeeds.
- [ ] The static proxy populates `player.health` / `player.ammo` and updates `intro.flashColor` each frame.
- [ ] `UiReadSnapshot` carries a resolved slot-value snapshot captured once per frame after game logic; the renderer reads slot values only from the snapshot, never the live table.
- [ ] A `text` node bound to `player.health` renders the slot's current value through its format template; a `panel` whose `fill` is bound to `intro.flashColor` renders the slot's color.
- [ ] The gameplay `UiTree` is retained on the renderer across frames. An appearance-only bound change (the color flash) refreshes the draw list **without** a taffy relayout — the tree's recompute counter does not increment. A layout-affecting bound change (text content that re-measures) **does** trigger relayout — the counter increments.
- [ ] Value diffing is subscriber-aware: a slot with no binding in the tree changing value invalidates nothing (no draw-list rebuild, no relayout).
- [ ] After the flash settles to a constant color, a no-change frame performs no draw-list rebuild and no relayout (the dirty-gate short-circuits in production — the B follow-up is closed).
- [ ] A `persist: true` slot round-trips: an engine-side write, a save, and a restart restore the value over the declared default; a non-persist slot does not serialize; a persisted entry with an unknown name or mismatched type is ignored with a warning.
- [ ] The generated TS and Luau typedefs express `StateValue<T>` as a generic branded type.
- [ ] The demo renders `player.health` / `player.ammo` as text and a subtle same-hue `panel` flash for ~3 s, then solid. Verification reuses A/B's approach: pure-CPU draw-list / diff / recompute-counter assertions plus a manual run per the project build/run commands — no new golden image.

## Tasks

### Task 1: Slot table, `defineState` primitive, validation, branded `StateValue<T>`
The scripting substrate. Add a `SlotTable` engine-global registry as a field on `ScriptCtx` (`Rc<RefCell<…>>`, mirroring `data_registry` — survives level unload, never cleared). Define the serde slot-schema types (per-slot `type` / `default` / `range` / `persist` / `readonly`, plus `values` for enum); discriminate the slot type by its `type` tag. Add a `scripting/primitives/state.rs` domain module exporting `register_state_primitives(registry, ctx)`, called from `register_all` (`primitives/mod.rs`); register `defineState` with `.scope(ContextScope::DefinitionOnly)`. The primitive receives the schema as a VM value, crosses the bridge via `js_to_json` / `lua_to_json` → `serde_json::from_value`, validates (finite/range, Rust-side, `LightDescriptor::validate` precedent), and writes slots into the table. Add the engine-side registration call for `player.*` (readonly, engine-owned) at init through the same table API. Extend the typedef generator so `StateValue<T>` emits as a generic branded type (the `TypeShape::Brand` emitter is non-generic today — `typedef.rs`). Pure data + scripting; no rendering. Produces the Boundary Inventory.

### Task 2: Static proxy
An engine-side writer (owned by `App`, holding its existing `ScriptCtx` clone) that each frame sets `player.health` / `player.ammo` to fixed demo values and computes `intro.flashColor` from a level-load timer: toggle between two same-hue RGBA endpoints every 500 ms for 3000 ms, then hold the solid endpoint. `intro` is a modder-declared slot (declared by the demo mod in Task 7) — the proxy writes it engine-side (mod writes deferred). Depends on Task 1's table + write API.

### Task 3: Published read handle
Extend `UiReadSnapshot` (`render/ui/mod.rs`) with a resolved slot-value snapshot field. `App` builds the snapshot from the slot table after game logic and before render, passing it through the existing `set_ui_snapshot` path. The snapshot **clones** the resolved values (a once-per-frame copy) so the renderer never touches the live table — preserves the renderer/game-logic boundary. Depends on Task 1's table.

### Task 4: Descriptor `bind` + value resolution
Add an optional `bind` to `TextWidget` (slot name + optional single-`{}` format template; resolves to `content`) and `PanelWidget` (slot name resolving to `fill`, reading an array/color slot). Wire camelCase per the Boundary Inventory. At draw-data build, resolve bound fields from the snapshot's slot values (number/bool/string/enum → formatted text; array → `[f32; 4]` fill). Unbound widgets keep their literal field. Threads the snapshot's slot values into the `layout_tree` / `build_draw_data` path (today `record_splash_ui` and the gameplay path call `self.ui.layout_tree(&tree, viewport, &image_sizes)` — add the slot snapshot as an input). Depends on Task 1 (types) + Task 3 (snapshot values).

### Task 5: Retained `UiTree` + subscriber-aware diffing → relayout/redraw split
Hold the gameplay `UiTree` on the `Renderer` (today `layout_tree` builds a fresh `UiTree::from_descriptor` every frame — `render/ui/mod.rs`). Rebuild the tree only on descriptor structural change or viewport resize; otherwise reuse it. Record, per bound node (in the tree's `NodeContext`), which slot it binds and that field's last resolved value. Each frame, diff only bound slots (subscriber-aware) against the new snapshot: a layout-affecting change (text content → re-measure) marks the node dirty so taffy relayouts; an appearance-only change (panel fill / text color) refreshes the draw list with no relayout. Split the existing `build_draw_data` so layout-compute stays gated (the `viewport_changed || structural_change` condition, extended with the value-driven dirty mark) while draw-data collection runs each frame from cached layout reading live snapshot values. The recompute counter must not increment on an appearance-only frame. Depends on Tasks 3 + 4.

### Task 6: Persisted-slot save wire format
Define and implement the `persist: true` save format: a versioned flat JSON map of dotted slot name → value, written to a single working-directory-relative path. Load-on-start applies persisted values over declared defaults *after* slot declaration, type-checked (mismatched/unknown ignored with a warn). Save-on-shutdown serializes only `persist: true` slots' current values. Exercised via the engine-side write API (mod-facing `setState` is deferred, so the round-trip test writes engine-side). Depends on Task 1 (the `persist` flag + table). Independent of Tasks 3–5.

### Task 7: Demo screen + CPU test gate
A Rust `build_demo_descriptor` (B's `build_splash_descriptor` precedent) emitting a tree that binds `player.health` / `player.ammo` to `text` nodes and `intro.flashColor` to a `panel` `fill`. A reference demo mod in TS and Luau declaring `intro` (writable) and `audio` (persist) via `defineState`, with a parity contract test (both runtimes register equivalent slots). Extend A/B's CPU assertion harness: bind resolution, subscriber-aware diff, the appearance-only-no-relayout vs content-change-relayout split, the post-settle no-recompute frame, and persist round-trip. Splash and egui untouched. Depends on Tasks 2, 4, 5.

## Sequencing

This is a layered foundation; most tasks consume the prior.

**Phase 1 (sequential):** Task 1 — slot table + `defineState` + brand. Blocks all.
**Phase 2 (concurrent):** Task 2 (proxy), Task 3 (read handle), Task 6 (persistence) — each depends only on Task 1, independent of one another.
**Phase 3 (sequential):** Task 4 — `bind` + resolution. Consumes Task 3's snapshot.
**Phase 4 (sequential):** Task 5 — retained tree + diffing. Consumes Task 4.
**Phase 5 (sequential):** Task 7 — demo + test gate. Consumes Tasks 2, 4, 5.

## Rough sketch

**Slot table.** A new `ScriptCtx` field, `Rc<RefCell<SlotTable>>`, mirroring `data_registry: Rc<RefCell<DataRegistry>>` (`ctx.rs`) — engine-global, populated in the definition context, never cleared across level loads. The slot value is a small tagged enum (`Number(f32) | Bool(bool) | Str(String) | Enum(u32 or interned) | Array(Vec<f32>)`) — cheaper diffing and clamping than carrying `serde_json::Value`. Each entry also stores the schema (type, default, range, `persist`, `readonly`) and a previous-frame value.

**`defineState` ingestion.** Mirror the gravity get/set precedent (`world.rs` `register_world_gravity`): the closure captures a `ScriptCtx` clone and writes through the `RefCell`. The schema arrives as a VM value → `js_to_json` / `lua_to_json` → `serde_json::from_value` into the schema struct → validate → insert. Engine-owned `player.*` uses the same insert path with a `readonly` + engine-owned marker. The `readonly` check rejects *script* writes (the future `setState` path, E/F); engine writes (proxy) bypass it.

**Brand generic.** `TypeShape::Brand { underlying }` emits `T & { readonly __brand: "T" }` non-generically (`typedef.rs`). Extend it (e.g. an optional generic parameter on the brand, or a dedicated generic-brand shape) so `StateValue<T>` emits `export type StateValue<T> = T & { readonly __brand: "StateValue" }` in TS and the Luau alias. The exact representation is the implementer's call within the constraint "the typedef expresses `StateValue<T>` generically."

**Read handle + resolution.** `UiReadSnapshot` gains a resolved-values map (cloned slot name → value). `App` fills it after game logic. `bind` on a widget is a slot-name string; resolution happens at draw-data build, reading the snapshot. No `StateValue` handle / named-leaf IR in C — that ergonomic layer is G1; C binds by string.

**Retained tree + split.** Hold `Option<UiTree>` (plus the descriptor it was built from) on the `Renderer` for the gameplay path. The tree's `NodeContext` carries the binding (slot name + target field) and last resolved value per bound node. Layout-compute stays behind the gate (`viewport_changed || structural_change || value_forced_dirty`); draw-data collection runs every frame from cached taffy rects, substituting current bound values. A color/fill change touches only draw data; a text-content change calls `taffy.mark_dirty(node)` so the gate fires. The splash path keeps rebuilding its descriptor each frame (transient, pre-state) — only the gameplay path retains.

**Key files.** `scripting/ctx.rs` (slot-table field), new `scripting/primitives/state.rs` (`defineState`), `scripting/primitives/mod.rs` (`register_all` wiring), `scripting/conv.rs` (bridge — reuse), `scripting/data_descriptors.rs` (validate precedent), `scripting/primitives_registry.rs` + `scripting/typedef.rs` (generic brand), `render/ui/mod.rs` (`UiReadSnapshot`, `layout_tree`), `render/ui/descriptor.rs` (`bind`), `render/ui/tree.rs` (`UiTree`, `NodeContext`, gate, draw-data split), `render/mod.rs` (`ui_snapshot`, `set_ui_snapshot`, gameplay path, retained tree), new `render/ui/demo.rs` or sibling (`build_demo_descriptor`), new persistence module, `content/dev/scripts/` (reference demo mod). Governing doc for wire/casing: `scripting.md`.

## Boundary inventory

The `defineState` schema and slot references cross Rust ↔ wire (JSON) ↔ JS/TS ↔ Luau. No FGD surface. Rust fields snake_case; wire/JS/Luau camelCase.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| primitive | `defineState` (registered fn) | n/a (call) | `defineState` | `defineState` |
| namespace | `String` arg | first positional string | `defineState("audio", …)` | `defineState("audio", …)` |
| slot type tag | enum by `type` | `type`: `"number"`/`"boolean"`/`"string"`/`"enum"`/`"array"` | same literals | same literals |
| default | `default` | `default` | `default` | `default` |
| range | `Option<[f32; 2]>` | `range` (`[min,max]`) | `range` | `range` |
| persist flag | `persist: bool` | `persist` | `persist` | `persist` |
| readonly flag | `readonly: bool` | `readonly` | `readonly` | `readonly` |
| enum values | `values: Vec<String>` | `values` | `values` | `values` |
| slot name (bind) | `String` | dotted, e.g. `"player.health"` | `"player.health"` | `"player.health"` |
| branded handle type | n/a (typedef only) | n/a | `StateValue<T>` | `StateValue<T>` |
| text bind | `Option<TextBind>` (`slot`, `format`) | `bind` (`{ slot, format }`) | `bind` | `bind` |
| panel fill bind | `Option<PanelBind>` (`slot`) | `bind` (`{ slot }`) | `bind` | `bind` |

`format` is a single-`{}` template (`"HP {}"`). The exact `TextBind` / `PanelBind` field set is the implementer's call within these casing rules.

## Save wire format

A new JSON (not PRL/binary) surface for `persist: true` slots:

- **File:** single working-directory-relative path (e.g. `state.json`). Per-user directory resolution is deferred (non-goal).
- **Shape:** `{ "version": <u32>, "slots": { "<dotted.name>": <value>, … } }`. Flat map; values match the slot's declared type (number → JSON number, boolean → bool, string/enum → string, array → JSON array of numbers).
- **Versioning:** integer `version`; a future format change bumps it. An unrecognized version is ignored with a warn (defaults stand).
- **Empty:** no persist slots → `"slots": {}`. Missing file → all defaults, not an error.
- **Restore order:** slots declared first (defaults applied), then persisted values overlaid. Unknown name or type mismatch → ignore + warn, never panic. Serializer: `serde_json` (the established path).

## Open questions

- **Primitive verb.** Spec uses `defineState` (matches `ui-layer.md` §9 and the roadmap). Owner second-guessed it (research §11.3): `defineState` names the category while it returns one `StateValue`. Alternative: `defineValue` / `trackValue` / a `state()` factory. The **type** `StateValue<T>` stays regardless; renaming the verb is a cheap pre-ship change. **Owner to confirm before promotion.**
- **Engine-owned registration path.** Recommended: the engine registers `player.*` through the same slot-table insert API `defineState` uses, with an engine-owned/readonly marker — one code path. Confirm vs. a dedicated engine-only path.
- **Generic-brand representation.** Extend `TypeShape::Brand` vs. a new generic-brand shape vs. hand-authored `StateValue<T>` in the SDK lib block. Implementer's call within the "emits generically" constraint.
- **Range vs. `max` authority.** The slot owns `range` (clamp/validate). The widget-side `max` (research §3, the bar's denominator) is a styling concern that lands with `bar` (F). C keeps authority on the slot.
- **Demo mod necessity.** C proves `defineState` via a reference mod declaring `intro`/`audio` (the descriptor stays Rust-built until G1). Confirm this split — declare-in-script, bind-in-Rust — reads cleanly as the C deliverable.
