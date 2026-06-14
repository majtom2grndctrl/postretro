# M13 G1b — UI SDK lifecycle: registration, always-on compose, `ui.localState()`

> Wave plan 2 of 2 (after **G1a**; both ship in one /orchestrate). Large plan — the full dynamic story.
> Prereqs: G1a (factories, bridge fns, typed handles, text alias), plus A–F/TW shipped. Grounding: `research/ui-layer.md` §18, `lib/ui.md` §1/§2/§3, `scripting/runtime.rs`, `scripting/staged_manifest.rs`, `scripting/data_descriptors.rs`, `scripting/data_registry.rs`, `render/ui/modal_stack.rs`, `render/ui/tree.rs`, `render/mod.rs`, `done/M13--ui-value-tweening/`.

## Goal

Connect the script-authored UI surface to the engine's register→VM-drop lifecycle, make registered trees render in production, and add the one stateful authoring primitive the static factory layer can't express: `ui.localState()`. G1a lets an author build a tree; G1b lets them register and theme it, see it render, and give modder-defined components per-instance presentation state. The VM drops after each registration pass; Rust owns the live UI every frame, no live VM (`scripting.md` §11).

The engine's UI runtime is further along than first assumed: the owned `UiTheme` + `theme_generation` + `set_ui_theme`, the production `layout_gameplay_tree` path, the retained-tree recompute gate, and `NodeContext` per-node storage **already exist and run in production** (`render/mod.rs:1344`/`3534`/`5441`). G1b therefore builds the *missing* pieces on top of them: the tiered registry, a general always-on compose step, the production callers for theme/font install, the manifest registration arms, and `localState`'s descriptor field + cell storage + SDK primitive. This is the convergence point G2 and BIS build on.

## Scope

### In scope

- **Manifest registration arms.** Extend `ModManifestResult` (`runtime.rs`), its script-facing `ModManifest` registered typedef + the parity guard (`primitives/mod.rs`), and `LevelManifest` (`data_descriptors.rs`, `{ reactions, crossings }`) with UI-tree / theme / font fields. Parse via G1a's `anchored_tree_from_js_value`/`_from_lua_value` wired into **all four** mod parsers (`run_mod_init_quickjs`, `run_mod_init_luau`, `manifest_from_js_value`, `run_staged_mod_init_luau`) **and** `LevelManifest::from_js_value`/`from_lua_value` — duplicate cold-boot vs. hot-reload paths that must both drain the new fields.
- **Tiered named-tree registry.** Add a scope-tier dimension (engine / mod) to the flat `UiTreeRegistry` (`modal_stack.rs`), with highest-precedence resolution (engine < mod) and a registration-time shadow warning. The per-level tier is **deferred**: the engine has a single-level lifetime today (`main.rs:2428`) with no runtime unload site, so there is no place to clear a level tier — `setupLevel` trees register into the mod-equivalent persistent tier for now, with the level tier landing when runtime level unload exists. Resolution stays the `&self` `ModalStack::tree` seam.
- **Always-on compose mechanism.** Extend the per-frame snapshot build (`main.rs` compose step) so every registered tree marked **always-on** is composed as a base layer (resolved through the tiered registry, so a mod tree shadows the engine HUD), with pushed modal-stack entries on top — all in the existing single-composition encode (`ui.md` §5). Today only `HUD_NAME` + pushed entries compose; this generalizes it to arbitrary always-on registered trees. Always-on is a registration attribute on the tree entry.
- **Theme + font install + script registration.** Drain mod-scope theme tokens into the existing owned `UiTheme` via the existing `Renderer::set_ui_theme` (adding the missing production caller; it bumps `theme_generation` already), reusing D's override-merge + `tree.rs` degradation. Add a `Renderer::register_ui_font(family, ttf)` install seam (glyphon is renderer-owned) and drain mod-scope font *assets* (family → TTF path, read as an asset) into it — distinct from theme font *tokens* (token → family) under the theme.
- **`ui.localState()` end-to-end.** (a) An additive `localState` cell-declaration field on the descriptor (`descriptor.rs`): a node declares a named local cell with an initial value; descendant widget binds may reference a local cell by name instead of a store slot. (b) Renderer-held per-node cell storage in the retained tree (a new `NodeContext` carrier or parallel per-`NodeId` map), positional, preserved across structurally-identical retained-diff reuse and discarded on structural rebuild. (c) The `ui`-namespaced SDK primitive (`sdk/lib/ui/state.{ts,luau}`, registered in `primitives_registry.rs`, auto-emitted to typedefs) returning a **distinct presentation handle** whose `.set()` emits a cell-write (not `setState`) and `.get()` a local bind reference. (d) A cell-write reaction for event-time writes (the presentation-only sibling of `setState`). Never writes the authoritative store (`ui.md` §3/§6). `State`-named (stored across frames) per `scripting.md` §11.
- **Modder-defined components as plain functions.** Confirm/test the convention (no `defineComponent`/decorator/inheritance); a modder component is a plain function returning a descriptor subtree, may call `ui.localState()`.
- **Lifecycle + render tests** over the production path.

### Out of scope

- Factories, bridge functions, typed handles, the text alias. → **G1a**.
- Runtime level unload and the per-level tree tier + clear-on-unload — no unload site exists today; deferred until one does.
- a11y preconditions, JSX → **G2**. Localization mechanism → deferred. New widget kinds / SE / BIS.
- Hot-reload cell preservation beyond the existing `staged_manifest`/`refresh_plan` model.

## Acceptance criteria

- [ ] A tree registered in `setupMod` resolves by name through the tiered registry after mod-init and renders in production on a **cold launch** (proving the cold-boot parser path was extended, not only the hot-reload one); a mod-tier tree registered under the HUD role shadows the engine HUD with a one-line shadow warning.
- [ ] A registered always-on tree (not the HUD, not pushed) composes into the per-frame snapshot and renders at its anchored placement on a normal launch; removing its registry entry removes it from the next frame. The compose stays a single `prepare`/composition (the `ui.md` §5 guard holds).
- [ ] A theme token registered from `setupMod` overrides the engine default in a rendered widget and bumps `theme_generation` (via the existing `set_ui_theme`); an unregistered token degrades (magenta / `body` / zero, warn-once) without panic. A registered font asset (family → TTF) loads through `Renderer::register_ui_font` and is usable by a `text` widget's `font`.
- [ ] UI registrations are drained from the manifest **before** the existing mod-init / data-script context drop (no new lifecycle stage); a frame then renders the registered UI with no VM resident.
- [ ] A modder component calling `ui.localState()` renders; its cell persists across frames at a stable value on a settled frame (the gameplay layer's retained-tree recompute counter does not increment) and resets to its declared initial after a structural rebuild. A descendant widget bound to the local cell displays its value.
- [ ] A cell-write (the `localState` `.set()` / cell-write reaction) updates the cell and the bound widget but leaves the authoritative store unchanged — verified by asserting no store slot is written.
- [ ] A modder component is callable with the same props-first-then-children shape as an SDK factory and nests inside SDK containers (a parity test mixes `VStack` with a modder component, passed through G1a's bridge).
- [ ] A malformed UI registration (theme token wrong type, tree that fails the bridge, duplicate non-shadowing name error) produces a named load-time diagnostic and is skipped — boot/level-load does not abort (`ui.md` §5).
- [ ] Generated typedefs include `ui.localState` and the manifest UI fields; the parity guard passes; `gen-script-types` reports no drift (regeneration incremental on G1a's committed output).

## Tasks

### Task 1: Manifest UI fields + dual-path bridge wiring
Add UI-tree/theme/font fields to `ModManifestResult` + the `ModManifest` typedef + parity guard (`primitives/mod.rs`) and a UI-tree field to `LevelManifest` (`data_descriptors.rs`). Drain via G1a's bridge fns in all four mod parsers (`run_mod_init_quickjs`, `run_mod_init_luau`, `manifest_from_js_value`, `run_staged_mod_init_luau`) and the two level parsers. `Depends on` G1a.

### Task 2: Tiered registry + always-on compose
Add the engine/mod tier dimension + shadow warning to `UiTreeRegistry` (`modal_stack.rs`); extend the `main.rs` compose step so always-on registered trees (tiered resolution) compose as base layers beneath pushed modal entries in the single composition. Level tier deferred. `Depends on` nothing (engine-side; concurrent-safe with Task 1).

### Task 3: Tree registration into the registry
Drain mod-scope (`setupMod`) and level-scope (`setupLevel`) trees into the tiered registry (both into the persistent tier today; level tier deferred). Mark always-on entries per their registration attribute. `Depends on` Tasks 1, 2.

### Task 4: Theme + font install + registration
Add the production caller of the existing `Renderer::set_ui_theme` to drain theme tokens (mod scope); add `Renderer::register_ui_font(family, ttf)` and drain font assets into glyphon. `Depends on` Task 1.

### Task 5: `ui.localState()` end-to-end
Additive `localState` cell-declaration field on `descriptor.rs`; renderer per-node cell storage (positional, diff-preserved, discarded on rebuild); the `ui.localState()` SDK primitive returning the distinct presentation handle; local-bind resolution to the nearest declaring ancestor cell; the cell-write reaction. `Depends on` G1a (handle ergonomics shape) and Task 2 (the retained-tree path the cell lives in — already production, extended here).

### Task 6: Modder-component convention + tests
Document plain-function components; add the lifecycle + render suite: cold-launch resolution/render, HUD-shadow + warning, always-on compose + single-composition guard, theme/font override, drain-before-drop, `localState` persist/reset/no-store-write, mixed SDK+modder tree. `Depends on` Tasks 2–5.

## Sequencing

**Prereq:** G1a complete.
**Phase 1 (concurrent):** Task 1 (manifest fields/parsers), Task 2 (registry tiers + compose) — disjoint (scripting vs render/main).
**Phase 2 (concurrent):** Task 3 (tree registration), Task 4 (theme/font), Task 5 (`localState`) — consume Phase 1.
**Phase 3 (sequential):** Task 6 (convention + cross-cutting tests).

## Rough sketch

**Already-built substrate (consume, don't rebuild):** the owned `UiTheme` + `theme_generation` + `set_ui_theme` (`render/mod.rs:1344`/`3534`), the production `layout_gameplay_tree` path (`render/mod.rs:5441`, threading owned theme + generation, single-composition encode), the recompute/draw counters (`tree.rs:390`/`403`), and positional `NodeContext` per-`NodeId` storage discarded on rebuild (`tree.rs:240`, the TW precedent). G1b adds callers and storage on top, not the substrate.

**Dual-parser reality.** `setupMod` is parsed by `run_mod_init_quickjs` (cold boot, `runtime.rs`) and `manifest_from_js_value` (hot-reload staged, `staged_manifest.rs`), with Luau twins; extending one wires one launch mode. `entity_descriptor_from_js`/`_from_lua` (`data_descriptors.rs`) are the field-reader precedent G1a's bridge fns follow.

**`ModManifest` is a typedef, not a Rust struct.** Rust is `ModManifestResult` (`runtime.rs:45`); `ModManifest` is the registered type (`primitives/mod.rs:407`) guarded by a parity test that fails if they diverge.

**Always-on compose** generalizes the current `main.rs` snapshot build (HUD-by-name + `modal_stack.entries()`): iterate always-on registered trees through the tiered registry as base layers, pushed modals on top, one composition. The HUD becomes one always-on tree (resolved through tiers so a mod can shadow it).

**`localState` is net-new state, not verbatim TW.** TW gave the *mechanism* (positional per-`NodeId` `NodeContext`, discarded on rebuild); `localState` adds a value carrier, a descriptor field to declare it, retained-diff preservation across structurally-identical reuse, and a cell-write path. Its handle is distinct from G1a's store handle: `.set()` writes the cell, never the store (`ui.md` §6).

**Key files:** `scripting/runtime.rs`, `scripting/staged_manifest.rs`, `scripting/data_descriptors.rs`, `scripting/data_registry.rs`, `scripting/primitives/mod.rs`, `scripting/primitives_registry.rs`, `scripting/typedef.rs`, `render/ui/modal_stack.rs` (tiers), `render/ui/descriptor.rs` (additive `localState` field), `render/ui/tree.rs` (cell storage), `render/mod.rs` (theme/font install, compose), `main.rs` (compose step, drains), `sdk/lib/ui/state.{ts,luau}`, `sdk/lib/index.ts`.

## Boundary inventory

Casing: Rust snake_case ↔ wire/JS/TS/Luau camelCase.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod UI trees | `ModManifestResult` field + `ModManifest` typedef | `"uiTrees"` | `setupMod` return key | same |
| level UI trees | `LevelManifest` field | `"uiTrees"` | `setupLevel` return key | same |
| always-on flag | tree registration attribute | `"alwaysOn"` | tree-entry field | same |
| theme tokens | drained via `set_ui_theme` → `UiTheme` | `"theme"` | `setupMod` return key | same |
| font assets | `Renderer::register_ui_font` → glyphon | `"fonts"` (family → TTF) | `setupMod` return key | same |
| local cell decl | additive `descriptor.rs` field | `"localState"` (name + init) | `ui.localState(init)` | `ui.localState(init)` |
| local cell value | per-node `NodeContext` cell (not serialized) | n/a | presentation handle | presentation handle |
| cell write | cell-write reaction (≠ `setState`) | `{"primitive":...}` | `handle.set(v)` | `handle:set(v)` |

## Decisions

- **G1-infra was dissolved; G1b owns the residual engine seams.** Review found the owned theme + `theme_generation` + `set_ui_theme` and the production `layout_gameplay_tree` path already exist; the residual work (registry tiers, always-on compose, theme/font callers, `localState` cell) is small and unifies with the dynamic story. Rejected: a separate infra plan (its two big items shipped; the rest splits `localState` across plans).
- **The level tree tier is deferred — no runtime unload exists.** The engine is single-level-lifetime (`main.rs:2428`) and `DataRegistry::clear` has no production caller; `setupLevel` trees register into the persistent tier today. The per-level tier + clear-on-unload land when runtime level unload does. Rejected: building a clear-on-unload with no site to attach to.
- **Tree override precedence is engine < mod (level deferred), last-wins + warn — the theme-override model.** A mod shadows engine built-ins (the reskin path); the warning fires at registration. Rejected: collision-as-error (blocks the intended reskin).
- **Always-on is a general compose role, not HUD-only.** Per owner direction, any registered always-on tree composes (HUD-shadow is the degenerate case). Rejected: HUD-shadow + pushed only.
- **`localState` uses a distinct presentation handle and a cell-write reaction, never `setState`.** Presentation-only is the decoupling payoff (`ui.md` §6); G1a's store handle writes the store, so `localState` needs its own. Positional per-`NodeId` `NodeContext` (the TW mechanism) keys the cell; reset-on-rebuild follows. Rejected: reusing G1a's store handle (store-writing semantics); an authored-string-id key (undefined for id-less instances).
- **`localState` requires an additive `descriptor.rs` field.** The cell declaration must be carried on a node so the renderer can allocate storage; this is the one descriptor change G1a left out. Additive (skip-serialized when absent), like `bind`/`tween`/`styleRanges`.

## Open questions

None. Structure and render scope resolved by owner; the level-tier deferral, the descriptor field, the distinct handle, and the dual-parser/install seams are resolved against source and recorded as Decisions.
