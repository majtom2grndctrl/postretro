# M13 G1b — UI SDK lifecycle: registration, always-on compose, `ui.localState()`

> Wave plan 2 of 2 (after **G1a**; both ship in one /orchestrate). Large plan — the full dynamic story.
> Prereqs: G1a (factories, bridge fns, typed handles, text alias), plus A–F/TW shipped. Grounding: `research/ui-layer.md` §18, `lib/ui.md` §1/§2/§3, `scripting/runtime.rs`, `scripting/staged_manifest.rs`, `scripting/data_descriptors.rs`, `scripting/reactions/system_commands.rs`, `render/ui/modal_stack.rs`, `render/ui/tree.rs`, `render/ui/text.rs`, `render/mod.rs`, `done/M13--ui-value-tweening/`.

## Goal

Connect the script-authored UI surface to the engine's register→VM-drop lifecycle, make registered trees render in production, and add the one stateful authoring primitive the static factory layer can't express: `ui.localState()`. G1a lets an author build a tree; G1b lets them register and theme it, see it render, and give modder-defined components per-instance presentation state. The VM drops after each registration pass; Rust owns the live UI every frame, no live VM (`scripting.md` §11).

The engine's UI runtime is further along than first assumed: the owned `UiTheme` + `theme_generation` + `set_ui_theme`, the production `layout_gameplay_tree` path, the retained-tree recompute gate, the `UiReadSnapshot.trees` layer vector, and per-node `NodeContext` runtime state **already exist and run in production** (`render/mod.rs:1344`/`3534`/`5441`, `mod.rs:314`, `tree.rs:240`). G1b builds the *missing* pieces on top: the tiered registry, a general always-on compose step, the production callers for theme/font install, the manifest registration arms, and `localState`'s descriptor field + app-side presentation-cell subsystem + SDK primitive. This is the convergence point G2 and BIS build on.

## Scope

### In scope

- **Manifest registration arms.** Extend `ModManifestResult` (`runtime.rs`), its `ModManifest` registered typedef + the parity guard (`primitives/mod.rs`), and `LevelManifest` (`data_descriptors.rs`) with UI-tree / theme / font fields. Parse via G1a's `anchored_tree_from_js_value`/`_from_lua_value` wired into **all four** mod parsers (`run_mod_init_quickjs`, `run_mod_init_luau`, `manifest_from_js_value`, `run_staged_mod_init_luau`) **and** `LevelManifest::from_js_value`/`from_lua_value` — duplicate cold-boot vs. hot-reload paths that must both drain the new fields.
- **Tiered named-tree registry.** Add a scope-tier dimension (engine / mod) to the flat `UiTreeRegistry` (`modal_stack.rs`), highest-precedence resolution (engine < mod), registration-time shadow warning. The per-level tier is **deferred**: single-level lifetime today (`main.rs:2428`), no runtime unload site, so `setupLevel` trees register into the persistent tier for now; the level tier + clear lands when runtime level unload exists. Resolution stays the `&self` `ModalStack::tree` seam.
- **Always-on compose mechanism.** Extend the per-frame snapshot build (`main.rs` compose step, currently `HUD_NAME` + `modal_stack.entries()` into `UiReadSnapshot.trees`) so every registered **always-on** tree composes as a base layer (tiered resolution, so a mod tree shadows the engine HUD), pushed modal entries on top, in the existing single-composition encode (`ui.md` §5; `UiReadSnapshot.trees` is already a layer `Vec` — additive). **Invariant:** base/always-on layers never contribute capture, focus, or text-entry — those derive from the pushed modal stack only (`modal_stack.rs` `top_capture_mode`/`active_*` read the pushed stack); base layers are forced passthrough/non-focusable regardless of declared `captureMode`. Always-on is a registration attribute on the tree entry.
- **Theme + font install + script registration.** Drain mod-scope theme tokens by deserializing the manifest `theme` field into a `ThemeDescriptor`, merging over `engine_default()` via `with_override`, and installing the merged `UiTheme` through the existing `Renderer::set_ui_theme` (which bumps `theme_generation`, gating retained rebuild) — adding the missing production caller; reuse D's override-merge + `tree.rs` degradation. Add a `Renderer::register_ui_font(family, ttf_bytes)` install seam delegating to the glyphon `FontSystem` (`text.rs` `build_font_system` registers compile-time `include_bytes!` faces today; this adds the **runtime** path) and a runtime TTF asset read (also net-new — no runtime font-file load exists today); drain mod-scope font *assets* (family → TTF path) into it, distinct from theme font *tokens* (token → family) under the theme.
- **`ui.localState()` end-to-end (id-addressed app-side presentation cells).** The author-driven write path cannot ride the TW `NodeContext` mechanism (tweens are render-derived, never reaction-written; reactions run app-side at the game-logic stage while the renderer is written one-way via the snapshot, and positional `NodeId`s are render-built). So `localState` is a small app-side presentation-state subsystem:
  - **(a) Descriptor field.** An additive `localState` declaration on `ContainerWidget` (`descriptor.rs`): a stable **scope id** plus named cells with initial values; descendant binds reference a cell by `{ local: "name" }`. Skip-serialized when absent, like `bind`/`tween`. The scope id is required when `localState` is used (author-supplied or SDK-stabilized), so the cell is addressable from **both** the app stage (writes) and the render stage (resolution).
  - **(b) App-side cell store.** A presentation-only map keyed by `(scopeId, cellName)`, seeded from declared initials when a tree is first registered/composed, cleared when the declaring scope id is no longer present. **Never the authoritative store** (`ui.md` §3/§6) — no schema, no persistence, no dotted-name namespace.
  - **(c) Cell-write reaction.** A new `SystemReactionCommand` arm (sibling of `SetState`) drained at the game-logic stage into the app-side cell store; `ui.localState()`'s handle `.set(v)` emits it. Distinct from `setState` (which writes the store).
  - **(d) Snapshot + resolution.** Resolved cell values ride an additive `UiReadSnapshot` field (the way bound slot values already flow), so the descriptor compared by the retained reuse gate (`mod.rs:902`) stays immutable and a cell write never forces a rebuild. The renderer resolves `{ local: "name" }` binds against the snapshot's cell values during `build_draw_data_retained`.
  - **(e) SDK primitive.** `ui.localState(init)` in `sdk/lib/ui/state.{ts,luau}` (registered in `primitives_registry.rs`, auto-emitted to typedefs), returning a **distinct presentation handle** (`.set()` = cell-write, `.get()` = local bind ref). `State`-named (stored across frames) per `scripting.md` §11.
- **Modder-defined components as plain functions.** Confirm/test the convention (no `defineComponent`/decorator/inheritance); a modder component is a plain function returning a descriptor subtree, and a component using `localState` declares a scope id on its root container.
- **Lifecycle + render tests** over the production path.

### Out of scope

- Factories, bridge functions, typed handles, the text alias. → **G1a**.
- Runtime level unload + the per-level tree tier + clear-on-unload — no unload site exists; deferred until one does.
- a11y preconditions, JSX → **G2**. Localization mechanism → deferred. New widget kinds / SE / BIS.
- Hot-reload cell preservation beyond the existing `staged_manifest`/`refresh_plan` model.

## Acceptance criteria

- [ ] A tree registered in `setupMod` resolves by name through the tiered registry after mod-init and renders in production on a **cold launch** (proving the cold-boot parser path was extended, not only hot-reload); a mod-tier tree registered under the HUD role shadows the engine HUD with a one-line shadow warning.
- [ ] A registered always-on tree (not the HUD, not pushed) composes into the per-frame snapshot and renders at its anchored placement on a normal launch; removing its registry entry removes it next frame. The compose stays a single `prepare`/composition (`ui.md` §5). A base/always-on layer never captures input or takes focus even if its descriptor declares `captureMode: "capture"` — capture/focus derive from the pushed modal stack only.
- [ ] A theme token registered from `setupMod` (merged over `engine_default` and installed via `set_ui_theme`) overrides the engine default in a rendered widget and bumps `theme_generation` so already-built trees pick it up; an unregistered token degrades (magenta / `body` / zero, warn-once) without panic. A registered font asset (family → TTF) loads at runtime through `Renderer::register_ui_font` and is usable by a `text` widget's `font`.
- [ ] UI registrations drain from the manifest **before** the existing mod-init / data-script context drop (no new lifecycle stage); a frame then renders the registered UI with no VM resident.
- [ ] A modder component calling `ui.localState()` renders, and a descendant widget bound `{ local: "name" }` displays the cell value. The cell value persists across frames at a stable value on a settled frame (the gameplay layer's recompute counter does not increment, because the live value rides the snapshot, not the compared descriptor). The cell value persists across a structurally-identical retained-diff reuse keyed by its scope id; it is discarded when its declaring scope id is no longer present.
- [ ] A `localState` `.set()` (the cell-write reaction, drained at game-logic stage) updates the cell and the bound widget but leaves the authoritative store unchanged — verified by asserting no store slot is written and the value is absent from the slot table.
- [ ] A modder component is callable with the same props-first-then-children shape as an SDK factory and nests inside SDK containers (a parity test mixes `VStack` with a modder component, passed through G1a's bridge).
- [ ] A malformed UI registration (theme token wrong type, tree that fails the bridge, a `local` bind referencing an undeclared cell) produces a named load-time diagnostic and is skipped/degraded — boot/level-load does not abort (`ui.md` §5).
- [ ] Generated typedefs include `ui.localState` and the manifest UI fields; the parity guard passes; `gen-script-types` reports no drift (regeneration incremental on G1a's committed output).

## Tasks

### Task 1: Manifest UI fields + dual-path bridge wiring
Add UI-tree/theme/font fields to `ModManifestResult` + the `ModManifest` typedef + parity guard (`primitives/mod.rs`) and a UI-tree field to `LevelManifest` (`data_descriptors.rs`). Drain via G1a's bridge fns in all four mod parsers and the two level parsers. `Depends on` G1a.

### Task 2: Tiered registry + always-on compose
Add the engine/mod tier + shadow warning to `UiTreeRegistry`; extend the `main.rs` compose step so always-on registered trees (tiered resolution) compose as base layers beneath pushed modal entries in the single composition, with the capture/focus invariant (base layers forced passthrough/non-focusable). Level tier deferred. `Depends on` nothing (engine-side; concurrent-safe with Task 1).

### Task 3: Tree registration into the registry
Drain mod-scope (`setupMod`) and level-scope (`setupLevel`) trees into the tiered registry (both into the persistent tier today). Mark always-on entries per their registration attribute. `Depends on` Tasks 1, 2.

### Task 4: Theme + font install + registration
Add the production caller: deserialize manifest `theme` → `ThemeDescriptor`, merge over `engine_default` → `set_ui_theme(merged)` (relying on its `theme_generation` bump). Add `Renderer::register_ui_font(family, ttf_bytes)` delegating to the glyphon `FontSystem`, plus the runtime TTF asset read, and drain font assets. `Depends on` Task 1.

### Task 5: `ui.localState()` end-to-end
Additive `localState` declaration on `ContainerWidget` (scope id + named cells + initials); the app-side `(scopeId, cellName)` presentation-cell store seeded/cleared by tree presence; a `SystemReactionCommand::CellWrite` arm drained at game-logic stage into that store; the resolved values published on an additive `UiReadSnapshot` field; render-time `{ local: "name" }` bind resolution against it; the `ui.localState()` SDK primitive + distinct presentation handle. Never writes the authoritative store. `Depends on` G1a (handle ergonomics shape) and Task 2 (the snapshot/compose path it extends).

### Task 6: Modder-component convention + tests
Document plain-function components (scope id required for `localState` users); add the lifecycle + render suite: cold-launch resolution/render, HUD-shadow + warning, always-on compose + single-composition + capture-invariant, theme/font override, drain-before-drop, `localState` persist/scope-clear/no-store-write, mixed SDK+modder tree. `Depends on` Tasks 2–5.

## Sequencing

**Prereq:** G1a complete.
**Phase 1 (concurrent):** Task 1 (manifest fields/parsers), Task 2 (registry tiers + compose) — disjoint (scripting vs render/main).
**Phase 2 (concurrent):** Task 3 (tree registration), Task 4 (theme/font), Task 5 (`localState`) — consume Phase 1.
**Phase 3 (sequential):** Task 6 (convention + cross-cutting tests).

## Rough sketch

**Already-built substrate (consume, don't rebuild):** the owned `UiTheme` + `theme_generation` + `set_ui_theme` (`render/mod.rs:1344`/`3534`), the production `layout_gameplay_tree` path + single-composition encode (`render/mod.rs:5441`/`5433-5495`), the `UiReadSnapshot.trees` layer vector (`mod.rs:314`), the recompute/draw counters (`tree.rs:390`/`403`), per-node `NodeContext` runtime state (`tree.rs:240`), and the `SystemReactionCommand`/`SetState` dispatch (`system_commands.rs`, drained in `App::dispatch_system_commands`). G1b adds callers, arms, and an app-side cell store on top.

**Dual-parser reality.** `setupMod` is parsed by `run_mod_init_quickjs` (cold boot) and `manifest_from_js_value` (hot-reload), with Luau twins; extending one wires one launch mode. `entity_descriptor_from_js`/`_from_lua` (`data_descriptors.rs`) are the field-reader precedent. `ModManifest` is the registered typedef; the Rust struct is `ModManifestResult` (parity-guarded at `primitives/mod.rs`).

**`localState` flows like a slot value, not like a tween.** The descriptor carries only the immutable declaration (scope id, cell name, init). The live value lives in an app-side `(scopeId, cellName)` map, written by the new `CellWrite` reaction at the game-logic stage, and published on the snapshot — so the retained reuse gate (`mod.rs:902`, a full descriptor `PartialEq`) never sees it and never rebuilds/resets on a write. This is the one piece the TW precedent does **not** cover (TW values are render-derived, never reaction-written). Cells survive by scope id across reuse and structurally-identical rebuilds; they clear when the scope id leaves the composed set.

**`localState` handle is distinct from G1a's store handle.** G1a's `.set()` is `setState` (store write); `localState`'s `.set()` is the cell-write reaction (presentation map). Same ergonomics, different target.

**Key files:** `scripting/runtime.rs`, `scripting/staged_manifest.rs`, `scripting/data_descriptors.rs`, `scripting/primitives/mod.rs`, `scripting/primitives_registry.rs`, `scripting/reactions/system_commands.rs` (+ `reaction_dispatch.rs`), `scripting/typedef.rs`, `render/ui/modal_stack.rs` (tiers), `render/ui/descriptor.rs` (additive `localState` on `ContainerWidget`), `render/ui/tree.rs` (`{local}` resolution), `render/ui/mod.rs` (snapshot cell field), `render/ui/text.rs` (`register_ui_font` → `FontSystem`), `render/mod.rs` (theme/font install, compose), `main.rs` (compose step, drains, dispatch), `sdk/lib/ui/state.{ts,luau}`, `sdk/lib/index.ts`.

## Boundary inventory

Casing: Rust snake_case ↔ wire/JS/TS/Luau camelCase.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod UI trees | `ModManifestResult` field + `ModManifest` typedef | `"uiTrees"` | `setupMod` return key | same |
| level UI trees | `LevelManifest` field | `"uiTrees"` | `setupLevel` return key | same |
| always-on flag | tree registration attribute | `"alwaysOn"` | tree-entry field | same |
| theme tokens | `ThemeDescriptor` → merge → `set_ui_theme(UiTheme)` | `"theme"` | `setupMod` return key | same |
| font assets | `Renderer::register_ui_font` → glyphon `FontSystem` | `"fonts"` (family → TTF) | `setupMod` return key | same |
| local cell decl | additive `ContainerWidget` field (scope id + cells + init) | `"localState"` | `ui.localState(init)` | `ui.localState(init)` |
| local cell value | app-side `(scopeId, cellName)` map → snapshot field | snapshot cell field | presentation handle | presentation handle |
| local bind | `{ local: "name" }` (resolved render-side) | `{"local":"name"}` | `.get()` | `:get()` |
| cell write | `SystemReactionCommand::CellWrite` (≠ `SetState`) | `{"primitive":...}` | `handle.set(v)` | `handle:set(v)` |

## Decisions

- **G1-infra was dissolved; G1b owns the residual engine seams.** Review found the owned theme + `theme_generation` + `set_ui_theme` and the production `layout_gameplay_tree` path already exist; the residual work is small and unifies with the dynamic story. Rejected: a separate infra plan.
- **`localState` is an id-addressed app-side presentation-cell subsystem, not the TW `NodeContext` mechanism.** Author-driven writes come from reactions (game-logic stage) and the renderer is one-way via the snapshot, so the cell must be app-side, addressed by an explicit scope id (positional `NodeId`s are render-built and unavailable at dispatch), written by a new `CellWrite` reaction, and published on the snapshot like slot values. The descriptor carries only the immutable declaration so the retained reuse gate (`mod.rs:902` `PartialEq`) never rebuilds on a write. Rejected: cell-in-renderer + reaction-write (no app→renderer channel, no dispatch-time address); reusing TW's positional `NodeContext` (no reaction-write path; tweens are render-derived).
- **Cells survive by scope id, not positional reset.** Because addressing is id-based, a cell persists across reuse and structurally-identical rebuilds and clears only when its scope id leaves the composed set — better semantics for authored instance state than positional discard, and the natural consequence of id-addressing. (Supersedes the earlier "positional, reset-on-rebuild" framing.)
- **`localState` declaration lives on `ContainerWidget` (the scope carrier), not every widget.** A cell is a subtree-scope concern ("nearest declaring ancestor"); the container is the clean single carrier. Rejected: a field on every widget (cells on childless leaves are meaningless).
- **`localState` uses a distinct presentation handle + `CellWrite` reaction, never `setState`.** Presentation-only is the decoupling payoff (`ui.md` §6); G1a's store handle writes the store. `State`-named because stored across frames (§11) — the `liveValue()` → `ui.localState()` rename M14 reserved.
- **Always-on base layers never contribute capture/focus/text-entry.** Capture/focus derive from the pushed modal stack only (`modal_stack.rs`); base layers are forced passthrough, so an always-on overlay can't steal input. Rejected: honoring a base layer's declared `captureMode` (an overlay would wrongly capture).
- **Theme install merges over `engine_default` and relies on `set_ui_theme`'s generation bump.** `set_ui_theme` takes a merged `UiTheme`, not a `ThemeDescriptor`; the caller merges first and the generation bump is what makes the override reach already-built trees. Rejected: passing the raw descriptor to `set_ui_theme` (wrong type; stale tokens without the bump).
- **The level tree tier is deferred — no runtime unload exists** (`main.rs:2428`; `DataRegistry::clear` has no production caller). `setupLevel` trees register into the persistent tier today; the per-level tier + clear land when runtime level unload does.
- **Tree override precedence is engine < mod (level deferred), last-wins + warn — the theme-override model.** A mod shadows engine built-ins (the reskin path). Rejected: collision-as-error.

## Open questions

None. The `localState` write path, scope-id addressing, theme merge, font runtime-load, capture invariant, and level-tier deferral are resolved against source and recorded as Decisions.
