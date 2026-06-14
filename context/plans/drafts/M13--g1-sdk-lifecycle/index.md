# M13 G1b — UI SDK lifecycle: registration + `ui.localState()`

> Wave plan 3 of 3 (after **G1a** and **G1-infra**; all ship in one /orchestrate).
> Prereqs: G1a (factories, bridge fns, typed handles, text alias), G1-infra (tiered registry, owned theme, wired render path), plus A–F/TW shipped. Grounding: `research/ui-layer.md` §18, `lib/ui.md` §1/§3, `scripting/runtime.rs`, `scripting/staged_manifest.rs`, `scripting/data_descriptors.rs`, `scripting/data_registry.rs`, `done/M13--ui-value-tweening/`.

## Goal

Connect the script-authored UI surface to the engine's register→VM-drop lifecycle and add the one stateful authoring primitive the static factory layer can't express: `ui.localState()`. G1a lets an author build a tree and G1-infra makes trees/themes render in production; G1b lets an author *register* them — themes and fonts at mod-init (engine-global), named UI trees at mod-init (mod tier) or level-load (level tier) — and gives modder-defined components per-instance presentation state. The VM drops after each registration pass; Rust owns the live UI every frame, no live VM (`scripting.md` §11).

This closes the convergence: G1a's factories + G1-infra's runtime spine + G1b's lifecycle let a mod author a complete HUD or menu in TypeScript or Luau, register and theme it, and see it render — the surface G2 and BIS build on. G1b builds no registry, no theme ownership, no render wiring (those are G1-infra); it builds the *script-facing* registration arms onto them.

## Scope

### In scope

- **Manifest registration arms.** Extend the `setupMod` result (`ModManifestResult` in `runtime.rs`, plus its script-facing `ModManifest` registered typedef and the parity guard in `primitives/mod.rs`) with UI-tree / theme / font fields, and `LevelManifest` (`data_descriptors.rs`, `{ reactions, crossings }`) with a UI-tree field. Parse them via G1a's `anchored_tree_from_js_value`/`_from_lua_value` bridge functions, wired into **both** the cold-boot parsers (`run_mod_init_quickjs` / `run_mod_init_luau`; `LevelManifest::from_js_value`/`from_lua_value`) **and** the hot-reload staged parsers (`manifest_from_js_value` / `run_staged_mod_init_luau`) — these are duplicate paths and both must drain the new fields or registration silently no-ops on one path.
- **Named-tree script registration.** Drain mod-tier trees (from `setupMod`) and level-tier trees (from `setupLevel`) into G1-infra's tiered registry — mod tier survives unload, level tier clears at the unload sweep beside `DataRegistry::clear` (which clears reactions + crossings). Resolution and shadow-precedence are G1-infra's; G1b only feeds the tiers.
- **Theme + font script registration.** Drain mod-tier theme tokens into G1-infra's owned `UiTheme` via `with_override` (bumping `theme_generation`), and register font *assets* (family name → TTF path, feeding glyphon) into the font table — distinct from theme font *tokens* (token → family). Mod-init scope.
- **`ui.localState()`.** A `ui`-namespaced SDK primitive (`sdk/lib/ui/state.{ts,luau}`, registered in `primitives_registry.rs`, auto-emitted to typedefs) that declares a per-instance presentation cell as a descriptor field at its node and returns a renderer-local handle of G1a's exported wrapper type. The renderer holds the cell as per-node `NodeContext` state in the retained `UiTree` — positional by taffy `NodeId`, discarded on structural rebuild — exactly the TW `NodeContext`/`last_resolved` mechanism. Presentation-only: it never writes the authoritative store (`ui.md` §3). `State`-named (stored across frames) per `scripting.md` §11.
- **Modder-defined components as plain functions.** Confirm and test the convention: a modder component is a plain function returning a descriptor subtree (no `defineComponent`/decorator/inheritance), indistinguishable from an SDK factory at the call site, and may call `ui.localState()`.
- **Lifecycle tests** over the wired production path.

### Out of scope

- The registry split, owned theme, and render-loop wiring themselves. → **G1-infra**.
- Factories, bridge functions, typed handles, the text alias. → **G1a**.
- a11y preconditions, JSX. → **G2**. Localization mechanism → deferred. New widget kinds / SE / BIS.
- Hot-reload cell preservation beyond the existing `staged_manifest`/`refresh_plan` model.

## Acceptance criteria

- [ ] A tree registered in `setupMod` resolves by name through G1-infra's registry after mod-init and still resolves across a level load **and** unload (mod tier survives). A tree registered in `setupLevel` resolves during that level and no longer resolves after unload (level tier cleared at the `DataRegistry::clear` site). Both work on a **cold launch**, not only under hot reload — proving both parser paths were extended.
- [ ] A registered tree of a composed role renders at its anchored placement on a normal launch (consuming G1-infra's wired frame-loop path); removing its tier entry removes it from the next frame.
- [ ] A theme token registered from `setupMod` overrides the engine default in a rendered widget and bumps `theme_generation`; an unregistered token degrades (magenta / `body` / zero, warn-once) without panic. A registered font asset (family → TTF) is usable by a `text` widget's `font`.
- [ ] UI registrations are drained from the manifest **before** the existing mod-init / data-script context drop (no new lifecycle stage); a frame then renders the registered UI with no VM resident.
- [ ] A modder component calling `ui.localState()` renders; its cell persists across frames at a stable value on a settled frame (the retained-tree recompute counter does not increment) and resets to its declared initial after a structural rebuild — the positional `NodeContext` contract.
- [ ] No UI-module path writes the authoritative store from a `localState` cell (a test mutates a `localState` value and asserts the bound store slot is unchanged).
- [ ] A modder component is callable with the same props-first-then-children shape as an SDK factory and nests inside SDK containers (a parity test mixes `VStack` with a modder component and passes it through G1a's bridge).
- [ ] A malformed UI registration (theme token of wrong type, tree that fails the bridge) produces a named load-time diagnostic and is skipped — boot/level-load does not abort (`ui.md` §5).
- [ ] Generated typedefs include the `ui.localState` signature and the manifest UI fields; the `ModManifest`/`ModManifestResult` parity guard passes; `gen-script-types` reports no drift.

## Tasks

### Task 1: Manifest UI fields + dual-path bridge wiring
Add UI-tree/theme/font fields to `ModManifestResult` (`runtime.rs`), the `ModManifest` registered typedef and parity guard (`primitives/mod.rs`), and a UI-tree field to `LevelManifest` (`data_descriptors.rs`). Drain them via G1a's bridge fns in **all four** parsers: `run_mod_init_quickjs`, `run_mod_init_luau`, `manifest_from_js_value`, `run_staged_mod_init_luau` (mod) and `LevelManifest::from_js_value`/`from_lua_value` (level). `Depends on` G1a.

### Task 2: Named-tree registration into the tiers
Drain mod-tier trees (setupMod) and level-tier trees (setupLevel) into G1-infra's tiered registry; the level tier clears at the unload sweep beside `DataRegistry::clear`. `Depends on` Task 1 and G1-infra.

### Task 3: Theme + font registration
Drain theme tokens into G1-infra's owned `UiTheme` via `with_override`; register font assets (family → TTF) into the glyphon font table. Mod-init scope. `Depends on` Task 1 and G1-infra.

### Task 4: `ui.localState()`
SDK primitive in `sdk/lib/ui/state.{ts,luau}` (registered in `primitives_registry.rs`, auto-emitted to typedefs) returning G1a's exported handle type; renderer side allocates the per-node `NodeContext` cell (positional, discarded on rebuild) following TW. Presentation-only. `Depends on` G1a (handle type) and G1-infra (the wired retained-tree path the cell lives in).

### Task 5: Modder-component convention + lifecycle tests
Document plain-function modder components and add the lifecycle suite: cold-launch tier survival/clearing, drain-before-drop, `localState` persistence/reset/no-store-write, mixed SDK+modder tree through G1a's bridge. `Depends on` Tasks 2–4.

## Sequencing

**Prereq:** G1a and G1-infra complete.
**Phase 1 (sequential):** Task 1 — manifest fields + parsers all later tasks drain.
**Phase 2 (concurrent):** Task 2 (tree tiers), Task 3 (theme/font), Task 4 (`localState`).
**Phase 3 (sequential):** Task 5 — convention + cross-cutting lifecycle tests.

## Rough sketch

**Dual-parser reality.** `setupMod` is parsed twice — `run_mod_init_quickjs` (cold boot, `runtime.rs`) and `manifest_from_js_value` (hot-reload staged lane, `staged_manifest.rs`) — with Luau twins (`run_mod_init_luau`, `run_staged_mod_init_luau`). Extending only one wires registration into one launch mode. Task 1 must touch all four (plus `LevelManifest`'s two). `entity_descriptor_from_js`/`_from_lua` are the per-field-reader precedent G1a's bridge fns follow.

**`ModManifest` is a typedef, not a Rust struct.** The Rust struct is `ModManifestResult` (`runtime.rs`); `ModManifest` is the script-facing registered type (`primitives/mod.rs`) guarded by a parity test that fails if the two diverge. New fields land on both.

**`localState` is a descriptor-declared cell, not a live closure.** `ui.localState(init)` emits a cell declaration at its node (like TW's `tween` is a descriptor field) and returns a renderer-local handle; the retained tree allocates the value as per-node `NodeContext` state, positional by taffy `NodeId`, rebuilt-from-descriptor (reset) on structural rebuild. This is the TW mechanism verbatim — no stable string id, no script-addressable identity, no VM-resident state. The decoupling payoff (`ui.md` §3): presentation-only, never a store write.

**Key files:** `scripting/runtime.rs`, `scripting/staged_manifest.rs`, `scripting/data_descriptors.rs`, `scripting/data_registry.rs`, `scripting/primitives/mod.rs`, `scripting/primitives_registry.rs`, `scripting/typedef.rs`, `render/ui/modal_stack.rs` + `tree.rs` (G1-infra's registry/cell, consumed), `sdk/lib/ui/state.{ts,luau}`, `sdk/lib/index.ts`.

## Boundary inventory

Casing: Rust snake_case ↔ wire/JS/TS/Luau camelCase.

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| mod UI trees | `ModManifestResult` field + `ModManifest` typedef | `"uiTrees"` | `setupMod` return key | same |
| level UI trees | `LevelManifest` field | `"uiTrees"` | `setupLevel` return key | same |
| theme tokens | drained to `UiTheme` via `with_override` | `"theme"` | `setupMod` return key | same |
| font assets | glyphon font table entry | `"fonts"` (family → TTF) | `setupMod` return key | same |
| local state | per-node `NodeContext` cell (not serialized) | descriptor cell-decl field | `ui.localState(init)` | `ui.localState(init)` |
| tree name | tiered registry key `String` | registration `name` | `name` | `name` |

(`"fonts"` registers font *assets* — family → TTF for glyphon — distinct from theme font *tokens* (token → family) which live under `"theme"`.)

## Decisions

- **`localState` is positional per-node `NodeContext`, not stable-id-keyed.** Corrects the prior draft: TW's `NodeContext` is positional in the taffy tree and rebuilt from the descriptor each structural rebuild; "keyed by stable node id" mischaracterized it. Positional keying still yields persist-across-frames + reset-on-rebuild, and needs no script-addressable identity (modder-component instances carry no authored `id`). Rejected: an authored-`id` key (undefined for id-less instances) and a VM-resident closure (violates declare-then-drop).
- **Both cold-boot and hot-reload parsers are extended.** `manifest_from_js_value` is the staged/hot-reload lane only; cold boot is `run_mod_init_quickjs`. Wiring one silently breaks the other launch mode. Rejected: extending only the bridge-adjacent `manifest_from_js_value` (the original framing — boot registration would no-op).
- **`"fonts"` registers font assets; theme font tokens stay under `"theme"`.** Asset registration (family → TTF for glyphon) is a different concern from a theme token map (token → family); conflating them muddies both. Rejected: folding fonts into the theme descriptor's font category.
- **UI registration mirrors the entity-types-vs-reactions scope split.** Mod tier survives unload; level tier clears at the `DataRegistry::clear` site (which clears reactions + crossings). Reuses the established lifecycle shape rather than inventing one.
- **`localState` never writes the store; it is `State`-named.** Presentation-only is the decoupling payoff (`ui.md` §6); event-time store writes go through `setState`. `State`-named because it is stored across frames (§11) — the `liveValue()` → `ui.localState()` rename M14 reserved.

## Open questions

None. Forks resolved by owner (render path wired in G1-infra; this plan renders end-to-end); the localState-keying and parser-path corrections are folded into Decisions.
