# M13 G1b — UI SDK lifecycle: registration + `ui.localState()`

> Wave plan 2 of 2 (sequenced after **G1a — SDK core**; both ship in one `/orchestrate`).
> Prereqs: G1a (factories + bridge), plus B–F/TW shipped. Grounding: `research/ui-layer.md` §18, `lib/ui.md` §1/§3, `scripting/runtime.rs`, `render/ui/tree_asset.rs`, `render/ui/theme.rs`, `done/M13--ui-value-tweening` (NodeContext precedent).

## Goal

Wire the script-authored UI surface into the engine's existing register→VM-drop lifecycle, and add the one stateful authoring primitive the static factory layer cannot express: `ui.localState()`. G1a lets an author *build* a descriptor tree; G1b lets them *register* it — themes and fonts at mod-init (engine-global, survive level unload), named UI trees at mod-init or level-load (mirroring the engine's entity-types-vs-reactions registry split) — and gives modder-defined components per-instance presentation state. The script VM drops after each registration pass; Rust owns the live UI every frame with no live VM, exactly as `scripting.md` §11's "scripts declare, Rust executes" mandates.

This closes the convergence: with G1a's factories and G1b's lifecycle, a mod can author a complete HUD or menu in TypeScript or Luau, register it by name, and theme it — the surface G2 (type-safety/a11y) and BIS (built-in screens) build on. The register→drop spine already exists (`runtime.rs`); G1b adds UI arms to it, it invents no new lifecycle.

## Scope

### In scope

- **UI manifest registration.** Extend the `setupMod` manifest (engine-global scope) with UI-tree, theme-token, and font registrations, and the `setupLevel` `LevelManifest` (per-level scope) with UI-tree registrations — mirroring how `entities` (global) and `reactions`/`crossings` (per-level) already split. Parsed through G1a's bridge.
- **Named-tree script registration.** Route manifest UI trees into the app-side named-tree registry alongside the boot `register_tree_from_disk` path (`render/ui/tree_asset.rs`): mod-scoped trees into the engine-global registry (survive unload), level-scoped trees into a per-level set cleared on level unload (the `data_registry.clear()` precedent). The render path keeps resolving trees **by name** — no layout builder call (`ui.md` §1).
- **Theme + font script registration.** Route manifest theme tokens and font registrations into the engine theme registry (`render/ui/theme.rs`), mod-init scope — the "script-facing registration arrives with the UI SDK" seam D left (`ui.md` §2). Per-token override merge and unknown-token degradation are unchanged (D's behavior).
- **`ui.localState()`.** A `ui`-namespaced SDK primitive declaring a per-component-instance presentation cell; the renderer holds the cell keyed by stable node identity in the retained `UiTree`, following the `NodeContext`/`last_resolved` per-node precedent from TW. Presentation-only and renderer-local — never writes the authoritative store (`ui.md` §3). `State`-named (stored across frames) per the `scripting.md` §11 naming rule.
- **Modder-defined components as plain functions.** Confirm and test the convention (no `defineComponent`, decorator, or inheritance): a modder component is a plain function returning a descriptor subtree, indistinguishable from an SDK factory at the call site (research §15), and may call `ui.localState()` for instance state.
- **Lifecycle tests.** Mod-scoped registration survives level unload; level-scoped is cleared; `localState` cells persist across frames, discard on structural rebuild, and never originate a store write.

### Out of scope

- The factory layer, deserialization bridge, handle ergonomics, text-alias chokepoint. → **G1a**.
- a11y compile-preconditions, discriminated-union narrowing, template-literal nav types, JSX. → **G2**.
- The localization mechanism (per-locale string tables). → deferred.
- New widget kinds, screen-space effects, built-in screen authoring, egui retirement. → **SE / BIS**.
- Hot-reload-specific cell-preservation policy beyond the existing `staged_manifest`/`refresh_plan` model — reuse, do not extend.

## Acceptance criteria

- [ ] A mod that registers a UI tree by name in `setupMod` makes that tree resolvable by name from the per-frame snapshot path after mod-init, and it remains resolvable across a level load **and** a level unload (engine-global scope). A tree registered in `setupLevel` is resolvable during that level and **gone** after unload (per-level scope, cleared like `DataRegistry::reactions`).
- [ ] A theme token registered from `setupMod` overrides the engine default for that token name and resolves in a widget that references it; an unregistered token still degrades per D's rules (unknown color → magenta, warn once) rather than panicking.
- [ ] After each registration pass the script VM context is dropped (no live VM during gameplay) — asserted the same way the existing `run_mod_init`/`run_data_script` drop is, e.g. the context is created and dropped within the call. A frame renders the registered UI with no VM resident.
- [ ] A modder component that calls `ui.localState()` renders, and its cell value persists across frames at a stable value when nothing changes (the retained-tree recompute counter does not increment on a settled frame). The same value is **discarded** (resets to its declared initial) after a structural tree rebuild — paired positive/negative cases.
- [ ] No code path in the UI module writes the authoritative store from a `localState` cell — verified by a test that mutates a `localState` value and asserts the bound store slot is unchanged (presentation-only contract, `ui.md` §3).
- [ ] A modder component is callable with the exact same `Props`-first-then-children shape as an SDK factory, and nests inside SDK containers (a parity test builds a tree mixing `VStack` with a modder component and deserializes it through G1a's bridge).
- [ ] A malformed UI registration (duplicate tree name, theme token of wrong type, tree that fails the bridge) produces a named load-time diagnostic and is skipped — boot/level-load does not abort (the `ui.md` §5 degrade-not-abort rule).
- [ ] Generated typedefs include the `ui.localState` signature and the manifest UI fields; `gen-script-types` reports no drift.

## Tasks

### Task 1: UI manifest fields + bridge routing
Extend the `setupMod` manifest result (`runtime.rs` `ModManifest`/`ModManifestResult`, engine-global) and the `LevelManifest` (per-level, today `{ reactions, crossings }`) with UI-tree / theme / font registration arrays, parsed via G1a's deserialization bridge in `manifest_from_js_value` (+ Luau twin). Drain points mirror the existing entity/reaction drains. `Depends on` G1a.

### Task 2: Named-tree registration scopes
Add a register-from-manifest path to the app-side named-tree registry beside `register_tree_from_disk` (`render/ui/tree_asset.rs`): mod-scoped trees into the engine-global registry; level-scoped into a per-level set cleared on level unload (mirror `DataRegistry::clear`). Duplicate-name and bridge-failure diagnostics. `Depends on` Task 1.

### Task 3: Theme + font script registration
Route manifest theme tokens and font registrations (mod-init scope) into the engine theme registry (`render/ui/theme.rs`), reusing D's per-token override-merge and unknown-token degradation. `Depends on` Task 1.

### Task 4: `ui.localState()` primitive + renderer cell
SDK side: the `ui`-namespaced `localState()` declaration in `sdk/lib/ui/state.{ts,luau}`, registered as a primitive (`primitives_registry.rs`, auto-emitted to typedefs). Engine side: a per-instance presentation cell held in the retained `UiTree`, keyed by stable node id, following TW's `NodeContext`/`last_resolved` per-node storage; presentation-only, discarded on structural rebuild. `Depends on` G1a (the component/factory model it scopes to).

### Task 5: Modder-component convention + lifecycle tests
Confirm/document plain-function modder components (no special machinery) and add the lifecycle test suite: scope survival/clearing, VM-drop, `localState` persistence/reset/no-store-write, mixed SDK+modder tree round-trip. `Depends on` Tasks 2–4.

## Sequencing

**Prereq:** G1a complete (factories, bridge, handle ergonomics).
**Phase 1 (sequential):** Task 1 — manifest fields all later tasks drain.
**Phase 2 (concurrent):** Task 2 (named-tree scopes), Task 3 (theme/font), Task 4 (`localState`) — independent consumers of the manifest + retained tree.
**Phase 3 (sequential):** Task 5 — convention + cross-cutting lifecycle tests over Tasks 2–4.

## Rough sketch

**Lifecycle reuse.** `run_mod_init` (`runtime.rs`) runs `setupMod` in a short-lived context, drains the result into engine-global registries, drops the context; `run_data_script` runs `setupLevel`, drains `LevelManifest` into per-level registries, drops. G1b adds UI arms to both drains — no new lifecycle stage. Engine-global vs. per-level is the same shape as `DataRegistry`'s `entities` (survive) vs. `reactions` (cleared); the named-tree registry and theme registry are the engine-global homes, the per-level tree set the cleared one.

**`localState` keying.** The modder component function runs at registration time and *declares* a cell with an initial value; the renderer allocates the cell against the node's stable id in the retained `UiTree` and persists it frame-to-frame (TW's `NodeContext` already stores per-node display state this way). There is no live VM read — the widget binds the cell like a renderer-local slot. A structural rebuild discards cells (in-flight values reset), consistent with `ui.md` §3's "structural tree rebuilds discard display state." This is the instance-identity model the mod-state-store plan deferred here because it "needs the SDK component model + lifecycle to scope to."

**`localState` vs. the store.** The Mod State Store is global, dotted-name-addressed, schema-validated, persistable, never cleared, and is the shared game-logic↔UI binding namespace. `ui.localState()` is per-instance, ephemeral, presentation-only, with no global name and no persistence — the same display-vs-authoritative boundary TW drew. Both are `State`-named because both are *stored* (§11); the difference is global authoritative vs. per-instance display.

**Key files:** `crates/postretro/src/scripting/runtime.rs`, `scripting/staged_manifest.rs`, `scripting/data_registry.rs`, `scripting/primitives_registry.rs`, `scripting/typedef.rs`, `render/ui/tree_asset.rs`, `render/ui/theme.rs`, `render/ui/tree.rs` (retained tree / cell storage), `sdk/lib/ui/state.{ts,luau}`, `sdk/lib/index.ts`.

## Boundary inventory

Casing rule (uniform): Rust snake_case ↔ wire/JS/TS/Luau camelCase.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod UI trees | `ModManifest` UI field | `"uiTrees"` | `setupMod` return key | same |
| level UI trees | `LevelManifest` UI field | `"uiTrees"` | `setupLevel` return key | same |
| theme tokens | theme registry entry | `"theme"` | `setupMod` return key | same |
| fonts | font registry entry | `"fonts"` | `setupMod` return key | same |
| local state | retained-tree cell (per-node) | n/a (not serialized) | `ui.localState(init)` | `ui.localState(init)` |
| tree name key | registry key `String` | tree registration `name` | `name` | `name` |

(Exact manifest field names and whether mod/level trees share the `uiTrees` key or use distinct keys — pinned during Task 1; both scopes route through the same bridge.)

## Decisions

- **`localState` cells are keyed by stable node id and discarded on structural rebuild.** Matches TW's `NodeContext` per-node storage and `ui.md` §3's rebuild-discards-display-state rule, and needs no live VM. Rejected: a VM-resident per-instance closure (violates declare-then-drop) and a globally-named cell (that is the Mod State Store, not instance state).
- **UI registration mirrors the entity-types-vs-reactions scope split exactly.** Mod-scoped trees/themes/fonts are engine-global and survive unload; level-scoped trees clear like `reactions`. Reuses the existing registry shapes rather than inventing a UI-specific lifecycle. Rejected: a single global scope (level trees would leak across levels).
- **`localState` is `State`-named, never `runtime`/`live`.** It is stored across frames (§11: "State means stored"); it is only the *scope* that is instance-local. This is the `liveValue()` → `ui.localState()` rename M14 reserved. Rejected: `ui.liveValue()` (ambiguous; "live" reads as computed/runtime).
- **`localState` never writes the store.** Presentation-only is the decoupling-seam payoff; event-time store writes go through `setState` (`ui.md` §6). Rejected: a `localState` that can mirror back into a slot.

## Open questions

- Do level-scoped trees override a mod-scoped tree of the same name, or is a name collision a diagnostic? Lean: collision is a named diagnostic; level overrides need an explicit opt-in if ever wanted.
- `localState` reset-vs-preserve under debug hot reload: reuse `staged_manifest`/`refresh_plan`'s slot-value-preserving model, or always reset cells? Lean: reset (cells are presentation-only; a reset flash on reload is acceptable) — confirm against the hot-reload UX.
- Whether fonts register only at mod-init or also per-level. Lean: mod-init only (fonts are global assets); revisit if a level needs a one-off face.
