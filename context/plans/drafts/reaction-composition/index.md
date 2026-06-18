# Reaction Composition

> Prerequisites: `in-progress/mod-map-catalog` (`defineMod` + the catalog whose `tags` classify levels — hard dependency; largely landed in source) and `done/runtime-level-lifecycle` (shipped — it relocated the install/commit code from `main.rs` into `startup/lifecycle.rs`, and recomposition-per-load is meaningful only in the multi-level world). Prerequisite for `drafts/mod-frontend-hub`'s death / level-flow handling.

## Goal

Let reactions be reused across levels without copy-paste. Add an engine-global **mod-global reaction tier** (declared in `defineMod`, surviving unload like entity types) plus an optional per-reaction **level-tag scope**. The per-level active reaction set is composed at load from `(mod-global matching the level's tags)` ∪ `(level-local)`. A level's classification tags come from its map-catalog entry (`mod-map-catalog`). This completes the tiering that entity types and UI trees already have — reactions are the odd one out today (per-level only, `scripting.md` §2/§10.4).

## Scope

### In scope

- `defineMod` gains `reactions` (and `crossings`) — engine-global *definitions*, committed at mod-init and surviving level unload, mirroring entity types.
- Optional per-reaction `levels` scope (string tags). **Absent = applies to every level** (global is the degenerate case of the selector — one mechanism, not two tiers).
- A level's tags come from its **map-catalog entry** (`mod-map-catalog`), read at install by the loading map's id/path — available before the data script runs. The engine retains the active level's tags for recomposition. The `setupLevel` return stays behavior-only (`{reactions, crossings?, uiTrees?}`, typedef.rs:868) — this plan adds nothing to it.
- Composition at level install: active reaction set = `(mod-global filtered by level tags)` ∪ `(level-local)`. **Additive union** — matches the existing fire-all-matching dispatch (`fire_named_event`, reaction_dispatch.rs:116). Same treatment for crossings. **Name collisions are not deduplicated**: if two active reactions share a `name`, all fire (the additive model; dedupe would be the override semantics deferred below). A debug `log::warn!` flags same-name collisions in the composed set as a likely authoring mistake.
- Durable definitions vs per-level activation: mod-global definitions live engine-global (survive unload); the active set clears on unload and recomposes on load — the same split entity *types* (global) and *instances* (per-level) already follow.
- Staged hot reload: mod-global reactions replace atomically (like `replace_entity_types`); the active set recomposes against the current level's retained tags. Failed/stale staged results preserve the prior set.
- SDK sugar: a group helper that stamps `levels` on many reactions at once (the "bundle"), plus generated typedefs for the new `defineMod` fields.

### Out of scope

- **Explicit override/suppression** of a lower tier's reaction. Composition is additive; disambiguate via disjoint scopes. The global-plus-scoped-collide case is documented guidance ("global = universal-only"), not an engine feature. Deferred until a concrete need.
- Per-level entity-type registration (entities stay engine-global via `defineMod`, unchanged).
- New reaction primitives or dispatch changes (`loadLevel`/`restartLevel`/`returnToFrontend` belong to `mod-frontend-hub`).
- Tag-match logic beyond set intersection (any-match). No AND/NOT predicates in v1.

## Acceptance criteria

- [ ] A reaction declared in `defineMod` with no `levels` scope fires on every level, with no re-declaration in any level script.
- [ ] A reaction scoped `levels: ["campaign"]` fires on a level whose catalog entry carries the `"campaign"` tag and does not fire on a level lacking that tag.
- [ ] A level composes an active set that is the union of mod-global reactions matching its catalog tags and its own `setupLevel` reactions; all fire on their events.
- [ ] Two levels with disjoint tags each receive only their scoped reactions — a campaign-scoped reaction never fires on a deathmatch-tagged level (no override needed).
- [ ] Mod-global reaction definitions are structurally equal (via `PartialEq`) before and after a level unload, while the per-level active set clears and recomposes — CPU test, mirroring entity-type survival.
- [ ] Staged hot reload replaces the mod-global reaction set atomically and recomposes the active set against the current level's tags; failed and stale staged results preserve the prior definitions.
- [ ] Crossings receive the same tiering and scoping as reactions.
- [ ] Two active reactions sharing a `name` both fire (additive union, no dedupe), with a debug warning logged.
- [ ] No new `unsafe`.

## Tasks

### Task 1: Durable mod-global reaction tier

Add engine-global definition storage to `DataRegistry` (data_registry.rs:19) — `global_reactions: Vec<ScopedReaction>` (and `global_crossings: Vec<ScopedCrossing>`), where `ScopedReaction { reaction: NamedReaction, levels: Vec<String> }` (and the crossing analogue) wraps the existing per-level type with its scope; the per-level `NamedReaction`/`CrossingDescriptor` stay unchanged. (Wrapper over extending the per-level type: scope is a tiering concern, not a field on every reaction — internal and reversible; an `Option<Vec<String>>` field is a defensible fallback if the wrapper proves noisy. Existing `DataRegistry` fields `reactions`, `crossings`, `entities`, `maps` — no collision.) Survive `clear()` (data_registry.rs:146, which clears only the per-level active sets, preserving `entities`/`maps`). Add `reactions`/`crossings` to `ModManifestResult` (runtime.rs:54) and `StagedManifest` (staged_manifest.rs:55); drain them in `run_mod_init_quickjs`/`run_mod_init_luau` (runtime.rs:1465/1669), mirroring the per-field `drain_maps_js`/`drain_maps_lua` sites (runtime.rs:1636/1789). Commit at mod-init mirroring the entity-type commit loop (`upsert_entity_type`, startup/lifecycle.rs:240; `replace_maps` at startup/lifecycle.rs:242 is the closer mirror for committing a global snapshot) and provide atomic replace mirroring `replace_entity_types` (data_registry.rs:132) for staged reload.

### Task 2: Read level tags from the catalog

A loading map's tags come from its `mod-map-catalog` entry — already resolved at install: `resolve_level_source` (startup/lifecycle.rs:428) stores the catalog entry's `tags` on the in-flight load (startup/lifecycle.rs:455; raw-path loads store `[]` at :471). No `LevelManifest`/`setupLevel` change (the return stays `{reactions, crossings?, uiTrees?}`). Retain the active level's tags on the app — `active_level_tags` is net-new; copy from `self.level_load.entry.tags` at install — so a staged reload can recompose. A map with no catalog entry (dev raw-path load) has empty tags and matches only unscoped global reactions.

### Task 3: Composition step

Turn `populate_from_manifest` (data_registry.rs:49, which takes a `LevelManifest`) into the composition entry: active `reactions` = `(global_reactions whose levels intersect the level tags — exact, case-sensitive string set-intersection — or whose levels is empty)` ∪ `level-local reactions`; same for crossings. `populate_from_manifest` *is* the data-script drain (startup/lifecycle.rs:809), so it must run before `progress_tracker.initialize` (startup/lifecycle.rs:810) and `crossing_detector.initialize` (startup/lifecycle.rs:818, after `crossing_detector.clear()` at :817) so both see the composed set. Add a recompose path callable on staged reload: the app's staged-reload reconcile handler (where `active_level_tags` lives) passes the retained tags into the `DataRegistry` recompose method.

### Task 4: SDK surface

Add `reactions`/`crossings` to the `defineMod` config and the per-reaction `levels` selector; add a group helper (`scopeReactions`) that stamps `levels` on many reactions. `scopeReactions` returns a plain stamped list in both runtimes — TS authors splice it with spread, Luau authors concatenate; no spread-parity (runtime symmetry is vocabulary and module IDs, not syntax — `scripting.md` §7). `ModManifest` is a *generated* type, not hand-written — add `reactions`/`crossings` to it via `ModManifestResult` (runtime.rs:54) plus the `register_type("ModManifest")` registration and the parity-test expected fields, then regenerate `sdk/types/postretro.d.{ts,luau}` (the drift-detection test enforces this). The hand-written SDK-lib blocks get the `scopeReactions` signature and the per-reaction `levels` field on `NamedReactionDescriptor`: `TS_SDK_LIB_BLOCK` (typedef.rs:680, type at typedef.rs:849) and `LUAU_SDK_LIB_BLOCK` (typedef.rs:1767, types at typedef.rs:2009-2012). Wire Luau exports of `scopeReactions` into `DATA_SCRIPT_FIELDS` (luau_prelude.rs:107) and `POSTRETRO_ROOT_MODULE_EXPORTS` (luau_prelude.rs:218). Pin the wire casing per the boundary inventory.

### Task 5: Tests and docs

CPU coverage: global-applies-everywhere, scoped-matches-only, disjoint-modes isolation, same-name-collision double-fire, definition survival across unload (assert via `PartialEq` — the stored reaction/crossing types must derive it), staged replace + recompose. Update `scripting.md` §2/§10 (reaction tiering, scope selector, composition) and `boot_sequence.md` §3/§5 — add the composition step to §3's numbered install order (it is the `populate_from_manifest` drain at stage 12, now composing global+level sets before `crossing_detector.initialize`) and document the durable-vs-active lifetimes.

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

Wire-casing note: the `"reactions"`/`"levels"`/`"crossings"` keys are enforced by literal-key reads in the JS/Luau drain helpers (data_descriptors.rs ~1022-1081), matching the `#[serde(rename_all = "camelCase")]` convention — not by a serde `rename` on the manifest struct. Both runtimes' drain helpers must use these exact keys (behavioral twins). Crossings reuse the identical `levels` field and casing as reactions; the `reaction scope` row covers both.

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
