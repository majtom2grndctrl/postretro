# Reaction Composition

> Prerequisites: `in-progress/mod-map-catalog` (`defineMod` + the catalog whose `tags` classify levels — hard dependency; largely landed in source) and `done/runtime-level-lifecycle` (shipped — it relocated the install/commit code from `main.rs` into `startup/lifecycle.rs`, and recomposition-per-load is meaningful only in the multi-level world). Prerequisite for `drafts/mod-frontend-hub`'s death / level-flow handling.

## Goal

Let reactions be reused across levels without copy-paste. Add an engine-global **mod-global reaction tier** (declared in `defineMod`, surviving unload like entity types) plus an optional per-reaction **level-tag scope**. The per-level active reaction set is composed at load from `(mod-global matching the level's tags)` ∪ `(level-local)`. A level's classification tags come from its map-catalog entry (`mod-map-catalog`). This completes the tiering that entity types and UI trees already have — reactions are the odd one out today (per-level only, `scripting.md` §2/§10.4).

## Scope

### In scope

- `defineMod` gains `reactions` (and `crossings`) — engine-global *definitions*, committed at mod-init and surviving level unload, mirroring entity types.
- Optional per-reaction `levels` scope (string tags). **Absent = applies to every level** (global is the degenerate case of the selector — one mechanism, not two tiers).
- A level's tags come from its **map-catalog entry** (`mod-map-catalog`), read at install by the loading map's id/path — available before the data script runs. The engine retains the active level's tags for recomposition. The `setupLevel` return stays behavior-only (`{reactions, crossings?, uiTrees?}`, typedef.rs:868) — this plan adds nothing to it.
- Composition at level install: active reaction set = `(mod-global filtered by level tags)` ∪ `(level-local)`. **Additive union** — matches the existing fire-all-matching dispatch (`fire_named_event`, reaction_dispatch.rs:116). Same treatment for crossings. **Name collisions are not deduplicated**: if two active reactions share a `name`, all fire (the additive model; dedupe would be the override semantics deferred below). A debug `log::warn!` flags same-name collisions in the composed set as a likely authoring mistake. Crossings get the same `levels` scoping and composition union but not the name-collision warn — `CrossingDescriptor` is keyed by slot, not name.
- Durable definitions vs per-level activation: mod-global definitions live engine-global (survive unload); the active set clears on unload and recomposes on load — the same split entity *types* (global) and *instances* (per-level) already follow.
- Staged hot reload: mod-global reactions replace atomically (like `replace_entity_types`); the active set recomposes against the current level's retained tags. A failed or stale staged result is a no-op: it preserves both the prior global definitions and the current active set.
- SDK sugar: `scopeReactions`, a group helper that stamps `levels` on many reactions at once (the "bundle"), plus generated typedefs for the new `defineMod` fields. Crossings set `levels` per entry; a parallel `scopeCrossings` batch helper is deferred until needed.

### Out of scope

- **Resolution of a colliding reaction** (override / suppression / dedupe / priority). Composition is additive; disambiguate via disjoint scopes. *Detecting* a same-name collision and emitting a debug warning is in scope; *resolving* it is deferred until a concrete need. Documented guidance for now: "global = universal-only".
- Per-level entity-type registration (entities stay engine-global via `defineMod`, unchanged).
- New reaction primitives or dispatch changes (`loadLevel`/`restartLevel`/`returnToFrontend` belong to `mod-frontend-hub`).
- Tag-match logic beyond set intersection (any-match). No AND/NOT predicates in v1.

## Acceptance criteria

- [ ] A reaction declared in `defineMod` with no `levels` scope fires on every level, with no re-declaration in any level script.
- [ ] A reaction scoped `levels: ["campaign"]` fires on a level whose catalog entry carries the `"campaign"` tag and does not fire on a level lacking that tag.
- [ ] A level composes an active set that is the union of mod-global reactions matching its catalog tags and its own `setupLevel` reactions; all fire on their events.
- [ ] Two levels with disjoint tags each receive only their scoped reactions — a campaign-scoped reaction never fires on a deathmatch-tagged level (no override needed).
- [ ] Mod-global reaction definitions are structurally equal (via `PartialEq`) before and after a level unload, while the per-level active set clears and recomposes — CPU test, mirroring entity-type survival.
- [ ] `DataRegistry` recompose is a pure function of current globals plus tags: after `replace_global_*` then recompose, the active set reflects the new globals; a failed/stale staged result skips the replace (existing early-return), leaving both the prior globals and the active set unchanged. Tested at the registry level, not the debug-gated commit flow.
- [ ] Crossings receive the same tiering and scoping as reactions.
- [ ] Two active reactions sharing a `name` both fire (additive union, no dedupe).
- [ ] The regenerated `sdk/types/postretro.d.{ts,luau}` match the registry (drift test `committed_sdk_types_match_current_registry`), and a runtime test confirms `scopeReactions(tags, list)` returns each reaction with `levels` set to `tags`. (Example `tsc --noEmit` type-check is a manual reviewer gate, not a `cargo test` assertion.)
- [ ] No new `unsafe` (grep/review gate, not a runnable test).

## Tasks

### Task 1: Durable mod-global reaction tier

Add engine-global definition storage to `DataRegistry` (data_registry.rs:19) — `global_reactions: Vec<ScopedReaction>` (and `global_crossings: Vec<ScopedCrossing>`), where `ScopedReaction { reaction: NamedReaction, levels: Vec<String> }` (and the crossing analogue) wraps the existing per-level type with its scope; the per-level `NamedReaction`/`CrossingDescriptor` stay unchanged. (Wrapper over extending the per-level type: scope is a tiering concern, not a field on every reaction — internal and reversible; an `Option<Vec<String>>` field is a defensible fallback if the wrapper proves noisy. Existing `DataRegistry` fields `reactions`, `crossings`, `entities`, `maps` — no collision.) Survive `clear()` (data_registry.rs:146, which clears only the per-level active sets, preserving `entities`/`maps`). Add `reactions`/`crossings` to `ModManifestResult` (runtime.rs:54) and `StagedManifest` (staged_manifest.rs:55); drain them in `run_mod_init_quickjs`/`run_mod_init_luau` (runtime.rs:1465/1669) by mirroring `drain_maps_js`/`drain_maps_lua` (called at runtime.rs:1636/1789, defined at data_descriptors.rs:4680/4954). Each new helper composes existing sub-parsers, not a flat object read: call `named_reaction_from_js`/`named_reaction_from_lua` (data_descriptors.rs:1115/2246) for the reaction body and `string_array_from_js`/`string_array_from_lua` (data_descriptors.rs:3979/5317; absent/null → empty `Vec`) for the `levels` key, then wrap both into `ScopedReaction`; crossings use `crossing_descriptor_from_js`/`crossing_descriptor_from_lua` (data_descriptors.rs:1165/2292). Commit at mod-init mirroring the entity-type commit loop (`upsert_entity_type`, startup/lifecycle.rs:240; `replace_maps` at startup/lifecycle.rs:242 is the closer mirror for committing a global snapshot) and provide atomic replace for staged reload — net-new `replace_global_reactions`/`replace_global_crossings` are plain infallible setters (`self.global_reactions = …`, no dedup; collisions are intentionally preserved), looser than `replace_entity_types` (data_registry.rs:132) which dedups by canonical name. Exclude the new globals from `clear()` (data_registry.rs:146) so they survive unload.

### Task 2: Read level tags from the catalog

A loading map's tags come from its `mod-map-catalog` entry — already resolved at install: `resolve_level_source` (startup/lifecycle.rs:428) stores the catalog entry's `tags` on the in-flight load (startup/lifecycle.rs:455; raw-path loads store `[]` at :471). No `LevelManifest`/`setupLevel` change (the return stays `{reactions, crossings?, uiTrees?}`). Retain the active level's tags on the app — `active_level_tags` is net-new; source them from `self.level_load.as_ref()?.entry.tags` at install (`level_load` is the `Option` at main.rs:992; `entry: LevelLoadEntry` is always present for catalog and raw-path loads at startup/mod.rs:49; `tags` lives on `LevelLoadEntry` at startup/mod.rs:45) — so a staged reload can recompose. `entry.tags` is the catalog tags for catalog loads (lifecycle.rs:455) and `Vec::new()` (`[]`) for raw-path dev loads (lifecycle.rs:471); a map with no catalog entry matches only unscoped global reactions. Set `active_level_tags` during install ahead of the `populate_from_manifest` call (lifecycle.rs:809) so Task 3's composition and a later staged recompose both read it.

### Task 3: Composition step

Turn `populate_from_manifest` (data_registry.rs:49) into the composition entry. It currently takes only a `LevelManifest`; give it a `tags: &[String]` parameter, threaded from `active_level_tags` at the lifecycle.rs:809 call site. Composed active `reactions` = `(global_reactions whose levels intersect the level tags — exact, case-sensitive string set-intersection — or whose levels is empty)` ∪ `level-local reactions`; same for crossings. Write the union into `self.reactions`/`self.crossings` — the existing fields `fire_named_event` (reaction_dispatch.rs:116) and `crossing_detector.initialize` read — not a parallel field. `populate_from_manifest` *is* the data-script drain (startup/lifecycle.rs:809), so it runs before `progress_tracker.initialize` (:810) and `crossing_detector.initialize` (:818, after `crossing_detector.clear()` at :817) so both see the composed set. Emit a debug `log::warn!` when two entries in the composed set share a `name`. Add a `DataRegistry` recompose method — a pure function of the current globals plus supplied tags that rebuilds the active set. The staged-reload wiring spans two modules: the atomic global replace stays inside `commit_staged_manifest_result` (runtime.rs:476, a `ScriptRuntime` method with no access to `active_level_tags`); the recompose runs at the App call site (main.rs:2571) after that returns a committed result, reading `self.active_level_tags`. A failed or stale staged result returns early before the replace (existing behavior), so it is already a no-op preserving both the prior globals and the active set; the whole staged path is `#[cfg(debug_assertions)]`.

### Task 4: SDK surface

Add `reactions`/`crossings` to the `defineMod` config and the per-reaction `levels` selector; add a group helper (`scopeReactions`) that stamps `levels` on many reactions. Implement its runnable `scopeReactions(tags, list)` body in `sdk/lib/data_script.{ts,luau}` alongside `defineReaction` (the `DataScriptSdk.*` pattern, data_script.luau:230 / data_script.ts:155), and add the runtime `levels` field to the SDK `NamedReactionDescriptor` types there — adding the name to `DATA_SCRIPT_FIELDS`/exports without a defined body errors at prelude load (the loader errors if a listed export has no body). `scopeReactions` returns a plain stamped list in both runtimes — TS authors splice it with spread, Luau authors concatenate; no spread-parity (runtime symmetry is vocabulary and module IDs, not syntax — `scripting.md` §7). `ModManifest` is a hand-registered shared type: its field list is authored in the `register_type("ModManifest")` builder chain (`register_shared_types`, primitives/mod.rs:453-482), and the SDK typedef text is generated from that registered shape. Add `reactions`/`crossings` to that chain — follow how the existing `entities`/`maps` array fields register their `Vec<…Descriptor>` type token so the generator resolves it to the lib-block type — and to `ModManifestResult` (runtime.rs:54). Update the parity test `mod_manifest_registered_type_matches_mod_manifest_result` (primitives/mod.rs:548) in its three coupled spots: the registered chain, the `_shape_anchor` `ModManifestResult` literal (primitives/mod.rs:564), and the `expected_fields` slice (:573). Then regenerate `sdk/types/postretro.d.{ts,luau}` (the drift test `committed_sdk_types_match_current_registry`, typedef.rs:4036, enforces this). The hand-written SDK-lib blocks get the `scopeReactions` signature and the per-reaction `levels` field on `NamedReactionDescriptor`: in `TS_SDK_LIB_BLOCK` (typedef.rs:680) a single edit on the intersection type at typedef.rs:849 (which also backs level-local `LevelManifest.reactions`, so `levels` must be optional); in `LUAU_SDK_LIB_BLOCK` (typedef.rs:1767) the field goes on each of the three variants (`Progress`/`Primitive`/`Sequence` `NamedReactionDescriptor`, typedef.rs:2009-2011; the union at :2012 inherits it). Wire Luau exports of `scopeReactions` into `DATA_SCRIPT_FIELDS` (luau_prelude.rs:107) and `POSTRETRO_ROOT_MODULE_EXPORTS` (luau_prelude.rs:218). Pin the wire casing per the boundary inventory.

### Task 5: Tests and docs

CPU coverage: global-applies-everywhere, scoped-matches-only, disjoint-modes isolation, same-name-collision double-fire (assert the warn via `log_capture::capture`, scripting/reactions/log_capture.rs:49), definition survival across unload (assert via `PartialEq` — `NamedReaction`/`CrossingDescriptor`/`ReactionDescriptor` already derive it at data_descriptors.rs:81/110/33, so `ScopedReaction`/`ScopedCrossing` can derive it and the per-level types stay unchanged), staged replace + recompose tested at the `DataRegistry` level (call `replace_global_*` then the recompose method against given tags) — not the full debug-gated `commit_staged_manifest_result` flow. Update `scripting.md` §2/§10 (reaction tiering, scope selector, composition) and `boot_sequence.md` §3/§5 — add the composition step to §3's numbered install order (it is the `populate_from_manifest` drain at stage 12, now composing global+level sets before `crossing_detector.initialize`) and document the durable-vs-active lifetimes.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2 — both edit `runtime.rs` drains; sequence to avoid contention. Task 1 establishes the durable tier; Task 2 the tag input.
**Phase 2 (sequential):** Task 3 — composition consumes both the durable tier and the level tags.
**Phase 3 (sequential):** Task 4 — SDK matches the settled Rust/wire shapes.
**Phase 4 (sequential):** Task 5 — tests and docs.

## Boundary inventory

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| mod-global reactions | `DataRegistry.global_reactions: Vec<ScopedReaction>` | `"reactions"` (on manifest) | `reactions` | `reactions` |
| reaction scope | `ScopedReaction.levels: Vec<String>` (empty = all) | `"levels"` | `levels` | `levels` |
| level tags (read) | catalog entry `tags` (from `mod-map-catalog`) | — | — | — |
| crossings tier | `DataRegistry.global_crossings: Vec<ScopedCrossing>` | `"crossings"` | `crossings` | `crossings` |

Wire-casing note: the `"reactions"`/`"levels"`/`"crossings"` keys are enforced by literal-key reads in the JS/Luau drain helpers (data_descriptors.rs ~1022-1091), matching the `#[serde(rename_all = "camelCase")]` convention — not by a serde `rename` on the manifest struct. Both runtimes' drain helpers must use these exact keys (behavioral twins). Crossings reuse the identical `levels` field and casing as reactions; the `reaction scope` row covers both.

## Script syntax examples

```ts
// Proposed design
import { defineMod, defineReaction, scopeReactions, playSound } from "postretro";

export function setupMod() {
  return defineMod({
    name: "My Campaign",
    reactions: [
      defineReaction({ name: "levelLoad", steps: [playSound("ambient/hum")] }), // every level
      ...scopeReactions(["campaign"], [campaignDeath, objectiveTracking]),       // campaign levels only
      ...scopeReactions(["deathmatch"], [dmRespawn, scoreLimit]),                // deathmatch levels only
    ],
  });
}

// levels/e1m1.ts — behavior only; the map's tags live in the catalog
// (mod-map-catalog), so the campaign-scoped reactions compose in by tag.
export function setupLevel(ctx) {
  return { reactions: [thisMapsSetpiece], crossings: [] }; // map-unique behavior
}
```

Note: `playSound` is currently exported from the prelude/UI surface, not the TS root barrel (`sdk/lib/index.ts`); surfacing it from `"postretro"` as the example shows requires adding it to the root barrel and `TS_SDK_LIB_BLOCK`.

## Decisions

- **`levels` is a per-reaction field**, the engine model; `scopeReactions` is sugar that stamps it. AC, Scope, and the boundary inventory all commit to the per-reaction field.
- **Name collisions double-fire.** If two active reactions share a `name`, all fire — the additive model; deduplication would be the override/suppression semantics deferred out of scope. A debug `log::warn!` flags same-name collisions in the composed set. Resolves from project leanness plus the no-abort twin-parsing rule (`scripting.md` §1).
- **Storage is a wrapper, not a field on the per-level type.** `global_reactions: Vec<ScopedReaction>` where `ScopedReaction { reaction, levels }`; the per-level `NamedReaction`/`CrossingDescriptor` are untouched. Internal and reversible — an `Option<Vec<String>>` field is a defensible fallback if the wrapper proves noisy.
- **No Luau spread-parity.** `scopeReactions` returns a plain stamped list in both runtimes; Luau authors concatenate. Runtime symmetry is vocabulary and module IDs, not syntax (`scripting.md` §7).
